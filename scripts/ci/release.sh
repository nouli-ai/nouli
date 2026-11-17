#!/usr/bin/env bash
# CI release policy. Offline policy tests run through Cargo.
set -euo pipefail
version() { sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml; }
fail() { printf '%s\n' "$*" >&2; exit 1; }
notes() {
  awk -v version="$(version)" '
    $1=="##" {gsub(/\[|\]/,"",$2); if($2==version) {capture=1; next} if(capture) exit}
    capture && /^\[[^][]+\]:/ {exit}
    capture {print}
  ' CHANGELOG.md
}
check() {
  local selected actual crate heading
  selected=$(version)
  [[ "$selected" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || fail 'Invalid workspace version'
  for crate in nouli-core nouli-cli; do
    actual=$(awk -v crate="$crate" '$0=="name = \"" crate "\"" {found=1; next} found && /^version = / {gsub(/"/,"",$3); print $3; exit}' Cargo.lock)
    [[ "$actual" == "$selected" ]] || fail "Cargo.lock version mismatch for $crate"
  done
  heading=$(awk -v version="$selected" '$1=="##" {name=$2; gsub(/\[|\]/,"",name); if(name==version) print}' CHANGELOG.md)
  [[ $(printf '%s\n' "$heading" | wc -l | tr -d ' ') == 1 ]] || fail 'Duplicate release sections'
  [[ "$heading" =~ ^##[[:space:]]\[$selected\][[:space:]]-[[:space:]][0-9]{4}-[0-9]{2}-[0-9]{2}$ ]] || fail 'Expected one dated release section'
  notes | grep '^[[:space:]]*[-*] ' >/dev/null || fail 'Release notes must contain a change'
}
eligible_runs() {
  local sha=$1
  [[ "$sha" =~ ^[a-f0-9]{40}$ ]] || fail 'Invalid source commit'
  jq -er --arg sha "$sha" '[.workflow_runs[] | select(
    .head_sha == $sha and .status == "completed" and .conclusion == "success" and
    ((.event == "push" and .head_branch == "main") or .event == "workflow_dispatch")
  )]'
}
choose_run() {
  eligible_runs "$1" | jq -er 'first | .id // empty'
}
catalog() {
  jq -e --slurpfile targets scripts/ci/targets.json '
    [.artifacts[] | select(.name | startswith("candidate-"))] as $items |
    ($items | length) == ($targets[0] | length) and
    all($targets[0][]; .target as $target |
      any($items[]; .name == ("candidate-" + $target) and .expired == false))
  ' >/dev/null
}
find_run() {
  local runs run artifacts
  runs=$(gh api "repos/$GITHUB_REPOSITORY/actions/workflows/ci.yml/runs?head_sha=$1&per_page=100") || return 2
  while IFS= read -r run; do
    artifacts=$(gh api "repos/$GITHUB_REPOSITORY/actions/runs/$run/artifacts?per_page=100") || return 2
    if printf '%s' "$artifacts" | catalog; then printf '%s\n' "$run"; return; fi
  done < <(printf '%s' "$runs" | eligible_runs "$1" | jq -r '.[].id')
  return 1
}
resolve_run() {
  local sha=$1 tag=$2 run started failure attempt status
  if run=$(find_run "$sha"); then printf '%s\n' "$run"; return; else status=$?; fi
  [[ "$status" == 1 ]] || fail 'Cannot read candidate runs or artifacts'
  started=$(date -u '+%Y-%m-%dT%H:%M:%SZ')
  gh workflow run ci.yml --ref "$tag" -f run_all=true >&2
  for ((attempt=0; attempt<180; attempt++)); do
    sleep 15
    if run=$(find_run "$sha"); then printf '%s\n' "$run"; return; else status=$?; fi
    [[ "$status" == 1 ]] || fail 'Cannot read candidate runs or artifacts'
    failure=$(gh api "repos/$GITHUB_REPOSITORY/actions/workflows/ci.yml/runs?head_sha=$sha&event=workflow_dispatch&per_page=100" \
      --jq ".workflow_runs[] | select(.created_at >= \"$started\" and .status == \"completed\" and .conclusion != \"success\") | .html_url")
    [[ -z "$failure" ]] || fail "Candidate validation failed: $failure"
  done
  fail 'Timed out waiting for candidate validation'
}
verify() {
  local directory=$1 selected archive expected count=0
  selected=$(version)
  while IFS= read -r archive; do
    [[ -f "$directory/$archive" && -f "$directory/$archive.sha256" ]] || fail "Missing release asset: $archive"
    expected=$(awk '{print $1}' "$directory/$archive.sha256")
    [[ "$expected" =~ ^[a-f0-9]{64}$ ]] || fail 'Invalid archive checksum'
    [[ $(cat "$directory/$archive.sha256") == "$expected  $archive" ]] || fail 'Checksum names a different archive'
    if command -v sha256sum >/dev/null; then
      (cd "$directory" && sha256sum --check "$archive.sha256")
    else
      (cd "$directory" && shasum -a 256 --check "$archive.sha256")
    fi
    count=$((count+2))
  done < <(jq -r --arg version "$selected" '.[] | "nouli-\($version)-\(.target).\(.extension)"' scripts/ci/targets.json)
  [[ $(find "$directory" -type f | wc -l | tr -d ' ') == "$count" ]] || fail 'Unexpected release files'
}
case "${1:-}" in
  check) check ;;
  prepare)
    check
    [[ "${2:-}" == "v$(version)" ]] || fail 'Tag does not match workspace version'
    git merge-base --is-ancestor HEAD origin/main || fail 'Release commit must belong to main'
    notes > "${3:?release notes path required}"
    ;;
  choose-run) choose_run "${2:?source commit required}" ;;
  catalog) catalog ;;
  resolve-run) resolve_run "${2:?source commit required}" "${3:?tag required}" ;;
  verify) verify "${2:?artifact directory required}" ;;
  *) fail 'Expected check, prepare, choose-run, resolve-run, or verify' ;;
esac
