# Task T008 — README per SPEC-022 (impl-bounded documentation)

Read the full brief first: `.sdd/changes/001-agent-context-cli/briefs/task-T008-brief.md` (in your worktree). Everything there binds: task text, the AC-022.1 checklist, global constraints, no-go list, report contract.

## Addendum (binding)

- Authoritative sources, all inside your worktree: `.sdd/changes/001-agent-context-cli/design-source.md` (the design contract — §7 holds the six-bullet AGENTS.md snippet you must reproduce verbatim), `.sdd/changes/001-agent-context-cli/spec.md` (SPEC-022 / AC-022.1 checklist; SPEC-019 threat-model boundary for the threat-model section), and the implemented CLI itself.
- Accuracy rule: every command, flag, output line, and exit code you show must match the real binary. Build it (`cargo build`) and check `target/debug/agent-context --help` plus each subcommand's `--help` before writing usage text. Do not invent flags or output.
- The README is a reader-facing artifact: written for users and coding agents consuming the tool, standing alone without access to the SDD process. No specification leakage (no SPEC-/AC- identifiers, no task numbers, no references to this development process), no filler. English.
- Required sections per the task text: overview; config schema by example (the design-example TOML); agent usage protocol (the verbatim six-bullet AGENTS.md snippet plus the discover → inspect → get → `run --with` flow and the no-guessing rule); threat model (the no-secret output invariant including the `run`-target and command-provider-stderr carve-outs — accidental-leak prevention, not defense against a malicious local process); provider guidance (prefer `keychain`/`command` locally; `env` values are readable by any process that inherits the environment); the sensitive-field-check guardrail caveat; target-name discovery (`inject_as` via `credential list`, `?as=` via JSON `reference` / `get`); Windows support statement (behavior specified, not machine-verified in v1).
- No-go: `src/`, `tests/` — README.md only, plus your report.

## Verification (record actual output in your report)

- `grep -c '^' README.md` — non-trivial line count.
- `grep -n 'agent-context list --json' README.md` — the snippet's first command present.
- Every shown command spot-checked against the built binary (list which ones you ran).

## Worker Execution Boundary

You are a leaf executor: complete this task directly; never spawn subagents or delegate.

## Report

Write `.sdd/changes/001-agent-context-cli/reports/task-T008-report.md` per the brief's Report Contract (Status/Provider/Model header lines, ## Implemented, ## Verification, ## Files Changed, ## Concerns).
