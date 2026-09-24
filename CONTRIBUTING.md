# Contributing to Herdinator

Issues and bug reports are welcome. Pull requests are welcome too, but are
reviewed on a best-effort basis with no response or merge timeline.

Keep changes focused on Herdinator's goal: managing named Herdr workspaces from
strict tmuxinator-compatible configuration.

Before opening a pull request, run:

```sh
cargo fmt --check
cargo clippy -- -D warnings
cargo test
```

Include tests for behavior changes and describe the user-visible effect in the
pull request. Changes outside the project's scope may be declined.
