# Herdinator

Herdinator is a tmuxinator-compatible project manager for named
[Herdr](https://herdr.dev/) workspaces. A tmuxinator project maps to a Herdr
workspace, windows map to tabs, and panes remain panes.

## Status

This is an early prototype targeting Herdr 0.9.1. It supports the strict core
of tmuxinator configuration rather than silently ignoring unsupported fields.

## Project Management

Herdinator is for recurring projects, not one-off layouts. It stores named
project configurations, discovers local and global project files using
tmuxinator conventions, and starts an existing matching workspace instead of
creating a duplicate. Use `new`, `open`, `edit`, `copy`, `delete`, and `list`
to manage the project configuration lifecycle.

## Next Steps

Herdinator will extend named project management to saved Herdr SSH machines. A
project will be able to start locally or on a selected remote machine while
keeping the same tmuxinator-compatible workflow.

```sh
herdinator start ali-test
herdinator start ali-test --machine Workbox --no-attach
```

Remote project support is planned and not available yet.

## Install

Requirements:

- Rust stable
- Herdr 0.9.1 or newer

```sh
cargo install herdinator
```

Herdinator creates its global configuration directory and `sample.yml`
automatically on its first command. To initialize them explicitly:

```sh
herdinator init
```

`init` is idempotent and never overwrites existing files. Start the sample with
`herdinator start sample`.

To install the local checkout instead:

```sh
cargo install --path .
```

## Usage

```sh
herdinator new shop
herdinator start shop
herdinator start shop --no-attach
herdinator start -p ./project.yml
herdinator list
herdinator debug shop
herdinator stop shop
```

Global projects live in `$HERDINATOR_CONFIG`,
`$XDG_CONFIG_HOME/herdinator`, or `~/.config/herdinator`. With no project name,
`start` also checks `.herdinator.yml`, `.tmuxinator.yml`, and a global project
matching the current directory name.

## Configuration

```yaml
name: shop
root: ~/projects/shop

pre_window: mise trust
startup_window: development
startup_pane: editor
attach: true

windows:
  - development:
      layout: main-vertical
      panes:
        - editor: nvim
        - server:
            - cd api
            - cargo run
        - tests: cargo watch -x test
  - logs: tail -f var/app.log
  - shell:
```

Supported layouts are `main-vertical`, `main-horizontal`, `tiled`,
`even-horizontal`, and `even-vertical`. Command arrays are sent to the pane one
at a time, preserving tmuxinator's send-keys semantics.

Supported project fields are `name`, `root`, `pre_window`, `startup_window`,
`startup_pane`, `attach`, and `windows`. A window can use `root`, `layout`,
`panes`, and `focused_pane`. Unknown and unsupported fields are errors.

Herdinator also accepts a native `tabs` format for exact `right` and `down`
split trees; see `examples/sample.yml`. New projects always use the tmuxinator
format.

## Intentional MVP Limits

Hooks, ERB, custom tmux layout strings, sockets, synchronization, `append`, and
session import are not supported yet.

## Inspiration

Herdinator is inspired by [tmuxinator](https://github.com/tmuxinator/tmuxinator)
and [tmuxrs](https://github.com/beijaflor/tmuxrs).

## License

Licensed under the [Apache License, Version 2.0](LICENSE).
