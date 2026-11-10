# Changelog

User-affecting changes only: the CLI surface, flags and their defaults, presets, actions, and platform support. Internal
refactors, docs, and test work stay in `git log`; the design docs describe the current behavior with no history at all,
so this file is where a user learns what moved between two builds.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versioning follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Each release tag owns a dated section, and what has landed
on `main` since the last tag accrues under Unreleased.

## [Unreleased]

## [0.1.1] - 2026-11-10

### Fixed

- The `url` preset stops at non-ASCII punctuation and symbols, so a URL followed directly by `）。`, `、`, or `～` in
  CJK text no longer takes them and the rest of the sentence with it.
- The `url` preset leaves out trailing `.`, `,`, `:`, `;`, `!`, and `?`, so a URL that ends a sentence no longer takes
  the sentence's punctuation. The same characters inside a URL (`a.b?q=1`) are kept.

## [0.1.0] - 2026-10-25

Initial release, for Linux, macOS, and Windows.

[unreleased]: https://github.com/felis-terminal/pouncer/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/felis-terminal/pouncer/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/felis-terminal/pouncer/releases/tag/v0.1.0
