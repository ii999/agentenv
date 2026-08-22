# Task Report: T006

- Change ID: 002-002-config-write
- Recorded: 2026-08-22T09:34:06.561817+00:00
- Source: /Users/zhaiqifeng/Dev/agent-context/.sdd/changes/002-002-config-write/reports/task-T006-report.md
- Status: DONE

## Worker Result (verbatim)

# Task Report: T006 — README and agent-protocol updates

Status: DONE

## Deliverables

- `README.md`: intro reflects the write capability; CLI reference documents `init`/`set`/`unset` (typing, `--description`, `--create-profile`, validation-before-write, atomicity, no locking / last-writer-wins per EDGE-006) and `credential add` per provider with the add→reference→`credential set` ordering; agent usage protocol block gains the write rules; safety section carries the threat-model delta (CLI-writable injection topology, mitigations, no-echo rule).

## Verification

- `cargo build` clean; sections verified by read-through against the final flag surface.
