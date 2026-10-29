# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Add Homebrew installation through `standing-desks/tap/nouli` and notify the tap when stable releases are published.
- Add a checksum-verified download installer for Apple silicon macOS and ARM64 or x86_64 GNU/Linux.
- Build Linux x86_64 release archives alongside the existing ARM64 downloads.
- Add a one-line question syntax: `id: prompt [a, b]` for a choice, `id: prompt <a, b, c>` for an ordered score, and `id: prompt` for a yes/no question.

### Fixed

- Keep the user guide within the viewport on narrow screens, with long commands and tables scrolling within the content.

## [0.1.0] - 2026-10-29

### Added

- Add typed choice, noul, and score decisions through the Nouli CLI and Rust library.
- Add durable session histories, branching, blending, and workflow recovery.
- Add decision graph views in the terminal, and `nouli view --mermaid` for Mermaid flowcharts on GitHub and other Markdown renderers.
- Add provider configuration and `.env` credential loading.
- Add a website with a user guide and interactive walkthroughs for robotics, banking, venture investing, food recalls, cybersecurity, and energy planning. Each walkthrough shows its commands, recorded model outputs, and a decision graph that follows along as you read.
- License Nouli and its Rust packages under MIT, including the license in native archives.
- Add Linux and macOS ARM64 downloads, with draft releases prepared when a version tag is pushed.
