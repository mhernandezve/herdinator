# Herdinator Agent Guide

## Verify Changes

- Run `cargo fmt --check`, `cargo clippy -- -D warnings`, and `cargo test` before handing off Rust changes.
- Build or install locally with `cargo build --release` or `cargo install --path .`.
- Test behavior that talks to Herdr with `herdinator start -p examples/smoke.yml --no-attach`; this requires Herdr 0.9.1+ available as `herdr` (or set `HERDINATOR_HERDR_BIN`). Use `herdinator doctor` to inspect that dependency.
- Releases are automated with `release-please` (opens a version-bump PR on push to `main`). Merging the PR creates the `vX.Y.Z` tag and triggers the `release.yml` workflow.
- The release workflow builds cross-platform binaries with `cargo-dist` (Linux x86_64/aarch64, macOS x86_64/aarch64), uploads them to the GitHub Release, and publishes to crates.io using the repository secret `CARGO_REGISTRY_TOKEN`.

## Architecture

- `src/main.rs` owns CLI side effects; `src/cli.rs` declares the Clap surface.
- `src/config.rs` is the strict YAML boundary. It parses tmuxinator-style `windows` or native `tabs` into `ProjectPlan`; reject unsupported fields rather than ignoring them.
- `src/layout.rs` converts named tmuxinator layouts into binary `LayoutNode` split trees. Native `tabs` layouts are the only format that expresses exact `right`/`down` split ratios.
- `src/project.rs` applies a `ProjectPlan` through the `HerdrApi` trait. Keep Herdr command details in `src/herdr.rs` so project behavior remains unit-testable through `FakeApi`.

## Behavior Constraints

- Project roots and window roots must expand to existing directories during parsing; fixtures must therefore use real temporary directories.
- `pre_window` commands are prepended to every pane and command arrays execute one command at a time.
- `start` is idempotent only when an existing workspace matches both its configured name and first-tab root. Failed creation must close the newly created workspace.
- Attaching requires an interactive TTY and is prohibited from inside Herdr (`HERDR_ENV=1`); use `--no-attach` for automated checks.
- Global configs resolve from `HERDINATOR_CONFIG`, then `$XDG_CONFIG_HOME/herdinator`, then `~/.config/herdinator`; local lookup prefers `.herdinator.yml` then `.tmuxinator.yml`.
