---
status: accepted
date: 2026-11-11
---

# Move the website to its own repository

## Context

The website at https://nouli.ai/ lived in `site/` under [ADR 0026](0026-astro-site-stack.md). It has its own toolchain (Node, Astro, Playwright, Python recording tools), its own checks and reviewer, and its own Pages deployment. The rest of this repository is a Rust workspace whose rules discourage adding other languages and test runners. Site work kept needing exceptions to those rules, and site and product changes shared one CI and one review flow.

The site also publishes `/install.sh`, which belongs with the releases whose assets it downloads.

## Decision

Move the website to [nouli-ai/nouli.ai](https://github.com/nouli-ai/nouli.ai), extracted with its history. That repository owns the `nouli.ai` Pages domain, its CI, its agent rules and the frontend reviewer. ADR 0026 continues there as its ADR 0001.

Keep the installer here as `scripts/install.sh`, linted with ShellCheck in CI. When a stable release is published, `.github/workflows/notify-release.yml` sends a `nouli-release-published` dispatch to the Homebrew tap and to the website. The website's sync workflow copies `scripts/install.sh` from the release tag to `public/install.sh` and opens a pull request that merges itself once its checks pass. Both notifications authenticate as the `nouli-tap-updater` GitHub App. Its token is scoped to the two downstream repositories.

## Considered alternatives

- **Keep the site in `site/`.** This avoids a second repository, but the site's tooling and its exceptions to this repository's rules would keep growing.
- **Move `install.sh` with the site.** Its tests already lived there, but the script would drift from the release assets it names, and installer changes would no longer be versioned with releases.
- **Have the site build fetch `install.sh` from a pinned ref.** This needs no pull requests, but changes would ship without the site's checks running on them, and the live script would depend on network access during the build.

## Consequences

Product and website changes run through separate repositories, checks and reviewers. Installer changes go live on nouli.ai with the next stable release. An urgent fix can be published sooner by running the website's sync by hand with a `ref`. A release that changes asset names must keep `scripts/install.sh` in step, and the website's installer tests run on each sync. Releases up to v0.5.1 predate `scripts/install.sh`, so syncs from them do nothing.

This supersedes [ADR 0026](0026-astro-site-stack.md) for this repository.
