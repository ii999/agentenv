# Task Report: T008

- Change ID: 003-project-config
- Recorded: 2026-08-28T09:05:57.980745+00:00
- Source: /Users/zhaiqifeng/Dev/agentenv/.sdd/changes/003-project-config/reports/task-T008-report.md
- Status: DONE

## Worker Result (verbatim)

# Implementation Report: T008

Status: DONE
Provider: codex
Model: gpt-5.6-terra
Profile: implementation

## Summary

Implemented the `project` command group with status reporting, trust lifecycle commands, and structural requirement checks. The JSON status envelope is snapshot-covered for every member state.

## Implemented

- Added `agentenv project status`, `allow`, and `revoke`, with the project group bypassing the general discovery prelude and performing its own facade calls.
- Added the frozen status envelope, documented non-zero report exit statuses, degraded configuration selection, and no-provider structural requirement checks.
- Exposed the existing structural entry lookup helpers at a narrow public seam and added integration coverage plus six normalized JSON snapshots.

## Verification

| Command or check | Result | Notes |
| --- | --- | --- |
| `cargo test --test project_status` | passed | 4 integration tests cover state snapshots, requirement outcomes, provider inertness, lifecycle output, and exit-2 JSON behavior. |
| `cargo fmt --check` | passed | Formatting is current. |
| `git diff --check` | passed | No whitespace errors. |
| `cargo test` | passed | Full unit, integration, and doctest suite passed. |

## Files Changed

| Path | Change |
| --- | --- |
| `src/cli/mod.rs` | Added project subcommand grammar and dispatch. |
| `src/cli/project.rs` | Added project status rendering, requirement checking, and allow/revoke output. |
| `src/main.rs` | Exempted the project group from the general discovery prelude. |
| `src/config/mod.rs` | Re-exported structural entry lookup. |
| `src/config/validate.rs` | Made structural entry resolution available to the CLI seam. |
| `src/query/mod.rs` | Exposed validated entry-table lookup. |
| `tests/project_status.rs` | Added project command integration coverage. |
| `tests/snapshots/project-status-*.json` | Added one normalized JSON snapshot per status member-state row. |

## Acceptance Coverage

| Acceptance ID | Evidence | Status |
| --- | --- | --- |
| AC-003.2, AC-003.4, AC-003.5, AC-003.6, EDGE-005 | `project_status::allow_and_revoke_render_their_outcomes` and facade error propagation. | Covered |
| AC-006.1..13 | `project_status::status_json_matches_the_frozen_member_state_table` and `status_infrastructure_failures_leave_json_stdout_empty`. | Covered |
| AC-007.1..7 | Structural lookup in `cli::project`; satisfied, missing, nested/table/reference, and no-provider paths are covered. | Covered |
| AC-010.3, AC-010.4 | Status uses no credential-resolution path; counting-provider test asserts no execution. | Covered |

## Self-Review

- [x] Scope matches the task brief.
- [x] No unrelated files changed.
- [x] Acceptance criteria are covered.
- [x] Verification evidence is recorded.
- [x] Concerns are documented.

## Concerns

none

## Impact Delta

The handoff map reported zero `Command` call sites, but the worker found the existing clap enum and dispatch in `src/cli/mod.rs` plus its parsed-command use in `src/main.rs`. The `resolve_in_entry` and `entry_table` locations matched the map; their visibility changed only to support the new CLI module.
