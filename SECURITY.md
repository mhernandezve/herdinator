# Security Policy

## Reporting A Vulnerability

Report suspected vulnerabilities through [GitHub Private Vulnerability
Reporting](https://github.com/mhernandezve/herdinator/security/advisories/new).
Do not open a public issue for security-sensitive reports.

## Project Files Execute Commands

A project file is executable input, not inert configuration. When you run
`herdinator start`, the commands in `pre_window`, window `panes`, and native
`tabs` are sent to Herdr panes and run with your user's privileges and
environment. The same is true for a project file reached through
`.herdinator.yml` discovery without an explicit name.

Only `start` executes project commands. `herdinator debug`, `list`, `edit`,
`copy`, and `delete` parse and manipulate the file without running anything, so
`herdinator debug <project>` is the safe way to inspect a project file from an
untrusted source before starting it.
