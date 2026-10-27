# Contributing to Ferriprint

Thank you for considering a contribution. Ferriprint is licensed **AGPL-3.0-only**
(see `LICENSE`); contributions are accepted under the same license without additional
terms.

## Development setup

- Rust stable via [rustup](https://rustup.rs); the minimum supported Rust version is
  tracked in `Cargo.toml` (`rust-version`).
- Build and check:

  ```sh
  cargo fmt --all -- --check
  cargo clippy --locked --all-targets -- -D warnings
  cargo test --locked --all-targets
  ```

- `Cargo.lock` is committed and CI runs with `--locked`; include lockfile changes with
  dependency changes.
- Develop from `public-development`, not the retained historical private `main`.
  Read [the repository workflow](docs/repository-workflow.md) before carrying a
  change between the private canonical and public GitHub remotes. Never merge a
  private historical parent into public development.
- Dependency changes must regenerate `THIRD_PARTY_NOTICES.md` with
  `scripts/release-metadata.py` from complete `cargo metadata --locked` output.
  Preserve the full locked graph and verbatim packaged license/copyright texts.
  The build and CI reject a stale notice inventory.

## Code style

- Start every Rust source file with the license header:

  ```rust
  // SPDX-License-Identifier: AGPL-3.0-only
  // Copyright (C) 2026 Robert Ressl
  ```

- Never commit credentials, certificates, private keys, or proprietary vendor PPDs/driver
  files, and never log secrets or document contents.
- Configuration examples must not contain real credentials or hostnames.
- Keep the user-facing documentation (README, configuration reference) consistent with the
  code; update `CHANGELOG.md` under `Unreleased` for user-visible changes.

## Submitting changes

- Small, focused changes with a clear description of behaviour and motivation.
- Tests for behavior changes: prefer tests that exercise observable behavior, not
  implementation wiring.
- Security issues are **not** submitted through public issues — follow `SECURITY.md`.
