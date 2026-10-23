#!/bin/sh
# Test double for the Herdr CLI.
#
# The scenario is selected by a `mode` file next to the symlink used as the
# binary, so tests never execute bytes written at runtime (which races with
# concurrent forks and fails with ETXTBSY).
mode=$(cat "${0%/*}/mode")

case "$mode" in
grouping)
    case "$*" in
    "workspace list") printf '%s' '{"result":{"workspaces":[
        {"workspace_id":"w1","label":"one"},
        {"workspace_id":"w2","label":"two"},
        {"workspace_id":"w3","label":"empty"}]}}' ;;
    "pane list") printf '%s' '{"result":{"panes":[
        {"workspace_id":"w1","cwd":"/first"},
        {"workspace_id":"w1","cwd":"/second"},
        {"workspace_id":"w2","cwd":"/other"}]}}' ;;
    *) printf '%s' '{"result":{"panes":[]}}' ;;
    esac
    ;;
fallback)
    case "$*" in
    "workspace list") printf '%s' '{"result":{"workspaces":[
        {"workspace_id":"w1","label":"one"}]}}' ;;
    "pane list") exit 1 ;;
    "pane list --workspace w1") printf '%s' '{"result":{"panes":[
        {"workspace_id":"w1","cwd":"/fallback"}]}}' ;;
    *) exit 1 ;;
    esac
    ;;
failing)
    case "$*" in
    "workspace list") printf '%s' '{"result":{"workspaces":[
        {"workspace_id":"w1","label":"one"}]}}' ;;
    *) exit 1 ;;
    esac
    ;;
machine)
    case "$*" in
    "--machine box pane list") printf '%s' '{"result":{"panes":[]}}' ;;
    "--version") printf 'herdr 0.9.1\n' ;;
    *) exit 1 ;;
    esac
    ;;
remote-down)
    case "$*" in
    "--version") printf 'herdr 0.9.1\n' ;;
    "--machine box status server") printf 'status: not running\n' ;;
    *) exit 1 ;;
    esac
    ;;
*)
    exit 1
    ;;
esac
