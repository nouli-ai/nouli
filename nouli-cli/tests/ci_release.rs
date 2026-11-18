//! Exercise release selection and completeness policy without GitHub or publication.
#![cfg(unix)]
#[path = "support/child.rs"]
#[allow(dead_code, reason = "policy tests use only the fixture executable")]
mod child;
use serde::Deserialize;
use serde_json::json;
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
};

fn policy(mode: &str, arguments: &[&str], input: &str) -> Output {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    let mut child = Command::new("bash")
        .arg("scripts/ci/release.sh")
        .arg(mode)
        .args(arguments)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[derive(Deserialize)]
struct Target {
    #[serde(rename = "target")]
    triple: String,
    extension: String,
    rustflags: String,
}
fn targets() -> Vec<Target> {
    serde_json::from_str(include_str!("../../scripts/ci/targets.json")).unwrap()
}

#[test]
fn candidates_must_be_successful_validations_of_the_exact_approved_commit() {
    let sha = "a".repeat(40);
    for (event, branch, status, conclusion, source, accepted) in [
        ("push", "main", "completed", "success", sha.as_str(), true),
        (
            "workflow_dispatch",
            "v0.6.0",
            "completed",
            "success",
            sha.as_str(),
            true,
        ),
        (
            "pull_request",
            "main",
            "completed",
            "success",
            sha.as_str(),
            false,
        ),
        (
            "push",
            "feature",
            "completed",
            "success",
            sha.as_str(),
            false,
        ),
        ("push", "main", "completed", "failure", sha.as_str(), false),
        (
            "push",
            "main",
            "in_progress",
            "success",
            sha.as_str(),
            false,
        ),
        ("push", "main", "completed", "success", "other", false),
    ] {
        let input = json!({"workflow_runs":[{"id":42,"event":event,"head_branch":branch,"status":status,"conclusion":conclusion,"head_sha":source}]}).to_string();
        let output = policy("choose-run", &[&sha], &input);
        assert_eq!(output.status.success(), accepted, "{input}");
        if accepted {
            assert_eq!(output.stdout, b"42\n");
        }
    }
}

#[test]
fn candidate_catalog_requires_all_five_native_artifacts_and_rejects_expiration() {
    let list = targets();
    assert_eq!(list.len(), 5);
    let mut artifacts: Vec<_> = list
        .iter()
        .map(|t| json!({"name":format!("candidate-{}",t.triple),"expired":false}))
        .collect();
    let run = |items: &[_]| {
        policy("catalog", &[], &json!({"artifacts":items}).to_string())
            .status
            .success()
    };
    assert!(run(&artifacts));
    artifacts[0]["expired"] = json!(true);
    assert!(!run(&artifacts));
    artifacts[0]["expired"] = json!(false);
    artifacts.pop();
    assert!(!run(&artifacts));
    for target in list {
        assert_eq!(
            target.extension == "zip",
            target.rustflags.contains("+crt-static")
        );
    }
}

#[test]
fn promotion_checks_real_digests_missing_files_wrong_names_and_extra_files() {
    let directory = std::env::temp_dir().join(format!(
        "nouli-candidates-{}",
        nouli_core::identity::Id::generate()
    ));
    fs::create_dir(&directory).unwrap();
    let mut names = Vec::new();
    for target in targets() {
        let name = format!(
            "nouli-{}-{}.{}",
            env!("CARGO_PKG_VERSION"),
            target.triple,
            target.extension
        );
        let path = directory.join(&name);
        fs::write(&path, b"fixture archive").unwrap();
        let hash = Command::new("shasum")
            .args(["-a", "256"])
            .arg(&path)
            .output()
            .unwrap();
        assert!(hash.status.success());
        let hash = String::from_utf8(hash.stdout)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .to_owned();
        fs::write(
            directory.join(format!("{name}.sha256")),
            format!("{hash}  {name}\n"),
        )
        .unwrap();
        names.push(name);
    }
    let valid = || {
        policy("verify", &[directory.to_str().unwrap()], "")
            .status
            .success()
    };
    assert!(valid());
    let checksum = directory.join(format!("{}.sha256", names[0]));
    let original = fs::read_to_string(&checksum).unwrap();
    fs::write(&checksum, original.replace(&names[0], "unapproved-archive")).unwrap();
    assert!(!valid());
    fs::write(checksum, original).unwrap();
    fs::write(directory.join("extra"), b"unapproved").unwrap();
    assert!(!valid());
    fs::remove_file(directory.join("extra")).unwrap();
    fs::write(directory.join(&names[0]), b"tampered").unwrap();
    assert!(!valid());
    fs::remove_file(directory.join(&names[0])).unwrap();
    assert!(!valid());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn release_metadata_rejects_missing_empty_undated_and_duplicate_sections() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    let temporary = std::env::temp_dir().join(format!(
        "nouli-release-notes-{}",
        nouli_core::identity::Id::generate()
    ));
    fs::create_dir(&temporary).unwrap();
    for file in ["Cargo.toml", "Cargo.lock"] {
        fs::copy(root.join(file), temporary.join(file)).unwrap();
    }
    let version = env!("CARGO_PKG_VERSION");
    let dated = format!("## [{version}] - 2026-11-17\n\n- Release change\n");
    let undated = format!("## [{version}]\n\n- Other change\n");
    for (contents, accepted) in [
        (dated.clone(), true),
        (String::new(), false),
        (format!("## [{version}] - 2026-11-17\n"), false),
        (undated.clone(), false),
        (dated.replace("2026-11-17", "not-a-date"), false),
        (format!("{dated}\n{undated}"), false),
        (format!("{undated}\n{dated}"), false),
        (format!("{dated}\n{dated}"), false),
    ] {
        fs::write(temporary.join("CHANGELOG.md"), contents).unwrap();
        let status = Command::new("bash")
            .arg(root.join("scripts/ci/release.sh"))
            .arg("check")
            .current_dir(&temporary)
            .output()
            .unwrap()
            .status;
        assert_eq!(status.success(), accepted);
    }
    fs::remove_dir_all(temporary).unwrap();
}

fn github_responses(temporary: &std::path::Path, scenario: &str, sha: &str) {
    let run = |id, source: &str, conclusion: &str| json!({"id":id,"head_sha":source,"event":"push","head_branch":"main","status":"completed","conclusion":conclusion});
    let mut before: Vec<_> = targets()
        .iter()
        .map(|t| json!({"name":format!("candidate-{}",t.triple),"expired":scenario=="expired"}))
        .collect();
    if scenario == "missing" {
        before.pop();
    }
    let after: Vec<_> = targets()
        .iter()
        .map(|t| json!({"name":format!("candidate-{}",t.triple),"expired":false}))
        .collect();
    let source = if scenario == "wrong-sha" { "b" } else { sha };
    let after_conclusion = if scenario == "failed-recovery" {
        "failure"
    } else {
        "success"
    };
    for (name, value) in [
        (
            "runs-before",
            json!({"workflow_runs":[run(7, source, "success")]}),
        ),
        (
            "runs-after",
            json!({"workflow_runs":[run(8, sha, after_conclusion)]}),
        ),
        ("catalog-before", json!({"artifacts":before})),
        ("catalog-after", json!({"artifacts":after})),
    ] {
        fs::write(temporary.join(format!("{name}.json")), value.to_string()).unwrap();
    }
    if scenario == "failed-recovery" {
        fs::write(temporary.join("catalog-before.json"), "{\"artifacts\":[]}").unwrap();
    }
}

#[test]
fn recovery_uses_the_same_validation_at_the_exact_tag_and_never_dispatches_on_api_errors() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    let sha = "a".repeat(40);
    for scenario in [
        "complete",
        "missing",
        "expired",
        "wrong-sha",
        "api-error",
        "failed-recovery",
    ] {
        let temporary = std::env::temp_dir().join(format!(
            "nouli-recovery-{}",
            nouli_core::identity::Id::generate()
        ));
        fs::create_dir_all(temporary.join("bin")).unwrap();
        for tool in ["gh", "sleep"] {
            std::os::unix::fs::symlink(child::executable(), temporary.join("bin").join(tool))
                .unwrap();
        }
        github_responses(&temporary, scenario, &sha);
        let mut command = Command::new("bash");
        command
            .arg(root.join("scripts/ci/release.sh"))
            .args(["resolve-run", &sha, "v0.6.0"])
            .current_dir(&root)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    temporary.join("bin").display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .env("GITHUB_REPOSITORY", "nouli-ai/nouli")
            .env("FIXTURE_GITHUB", &temporary)
            .env_remove("FIXTURE_API_FAILURE");
        if scenario == "api-error" {
            command.env("FIXTURE_API_FAILURE", "1");
        }
        let output = command.output().unwrap();
        assert_eq!(
            output.status.success(),
            !matches!(scenario, "api-error" | "failed-recovery"),
            "{scenario}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        if matches!(scenario, "complete" | "api-error") {
            assert!(!temporary.join("dispatched").exists());
        } else {
            let arguments: Vec<String> =
                serde_json::from_slice(&fs::read(temporary.join("dispatched")).unwrap()).unwrap();
            assert_eq!(
                arguments,
                [
                    "workflow",
                    "run",
                    "ci.yml",
                    "--ref",
                    "v0.6.0",
                    "-f",
                    "run_all=true"
                ]
            );
        }
        if output.status.success() {
            assert_eq!(
                output.stdout,
                if scenario == "complete" {
                    b"7\n"
                } else {
                    b"8\n"
                }
            );
        }
        fs::remove_dir_all(temporary).unwrap();
    }
}
