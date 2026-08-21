# Tasks: [Feature Name]

## Source Artifacts

- Change ID: [change-id]
- Plan: [path]
- Spec: [path or specs directory]

## Execution Rules

- Use local files under `.sdd/changes/[change-id]/` for all workflow state.
- Mark a task complete only after its verification and task review pass.
- `[P]` means parallel-safe because the task touches independent files or subsystems.
- TDD is exceptional: only tasks marked `TDD: yes` use it, each citing a necessity trigger and a paired test-authoring task (see `docs/subagent-execution.md`, "Lightweight implementation policy" — binding). Implementers never self-decide TDD; if implementation reveals a necessity trigger mid-task, escalate to the controller.
- For a `TDD: yes` task, the paired test task's failing suite is checkpointed first and is a read-only contract for the implementer.
- Acceptance criteria are the primary gate.

## Authoring Rules

Write every task for a fresh implementer with zero session context (see `docs/authoring-discipline.md`):

- Exact file paths, exact names and signatures, exact commands with expected output.
- `Interfaces:` records the exact signatures a task consumes from earlier tasks and produces for later ones — an isolated implementer cannot see neighboring tasks.
- After `Interfaces:`, every task entry carries `Impact seeds:` and `No-go:` (required for `Dispatch: agent` implementation tasks; optional for `inline`).
  - Value is a comma-separated list; trim surrounding whitespace and surrounding backticks; empty items are dropped.
  - Sole-item `none` (case-insensitive) is the sentinel for no-existing-call-sites (seeds) or no-restricted-regions (no-go). `none` mixed with other items is malformed.
  - Seeds MUST be specific symbol names or entry points; No-go items MUST be repo-relative paths or directories. Vague phrases ("related modules", "the router code") are placeholder violations.
- No placeholder phrases (deferral markers, "handle edge cases", "write tests for the above", "similar to task N") and no references to names that no task defines — the full blocklist lives in `docs/authoring-discipline.md`.
- A `TDD: yes` implementation task is preceded by its paired test-authoring task with its own sequential ID: deliverable is the failing test suite plus the exact interface signatures it pins down; `Dispatch: agent` on a high-capability (orchestrator-equivalent) route, inline only with a logged downgrade reason; verification asserts the new tests fail for the intended reason (assertion failures, not collection errors). The implementation task references it via `tests: T0xx`; its brief lists the test files as read-only contract files (not as `No-go:` entries — the implementer must run them).
- Group tasks by spec phase in priority order; every group ends with a `Checkpoint:` stating what is demonstrably working and how to observe it.

## Dispatch Preference

Every task carries a `Dispatch:` field (`agent` / `inline`); the orchestrator treats it as a binding default. The tier default (full: `agent`; light: `inline`), the provider ladder, provider-host affinity, frontend and Codex `impl-ui` handling, and the departure bar live in `docs/subagent-execution.md` ("Dispatch preference"). Record a one-line reason whenever a task departs from its tier default.

Reviews run at Checkpoint granularity by default; add an optional `Review: per-task` line to a task to force a per-task review (high-risk triggers are listed in `docs/subagent-execution.md`, "Review granularity").

## Task Groups

### Group 1: Foundation

- [ ] T001 [Foundation] Establish project structure for [feature]
  - Files: [create/modify paths]
  - Depends on: none
  - Spec refs: [SPEC IDs]
  - Acceptance refs: [AC IDs]
  - Interfaces: Produces: [exact names/signatures later tasks call]
  - Impact seeds: [symbol names, or `none`]
  - No-go: [repo-relative paths, or `none`]
  - TDD: no
  - Dispatch: [agent | inline] ([one-line reason])
  - Verification: `[command]` — expected: [what output proves success]
  - Report: `.sdd/changes/[change-id]/reports/task-T001-report.md`

Checkpoint: [What is demonstrably working after this group and how to observe it.]

### Group 2: Phase 1 (P1, MVP) - [Name]

- [ ] T002 [Phase 1] Implement [deliverable]
  - Files: [create/modify paths]
  - Depends on: T001
  - Spec refs: [SPEC IDs]
  - Acceptance refs: [AC IDs]
  - Interfaces: Consumes: [exact signatures from T001]; Produces: [exact signatures for later tasks]
  - Impact seeds: [symbol names, or `none`]
  - No-go: [repo-relative paths, or `none`]
  - TDD: [no | yes ([necessity trigger]; tests: T0xx)]
  - Dispatch: [agent | inline] ([one-line reason])
  - Verification: `[command]` — expected: [what output proves success]
  - Report: `.sdd/changes/[change-id]/reports/task-T002-report.md`

- [ ] T003 [P] [Phase 1] Implement independent [deliverable]
  - Files: [create/modify paths]
  - Depends on: T001
  - Spec refs: [SPEC IDs]
  - Acceptance refs: [AC IDs]
  - Interfaces: Consumes: [exact signatures]; Produces: [exact signatures]
  - Impact seeds: [symbol names, or `none`]
  - No-go: [repo-relative paths, or `none`]
  - TDD: [no | yes ([necessity trigger]; tests: T0xx)]
  - Dispatch: [agent | inline] ([one-line reason])
  - Verification: `[command]` — expected: [what output proves success]
  - Report: `.sdd/changes/[change-id]/reports/task-T003-report.md`

Checkpoint: Phase 1 acceptance criteria [AC IDs] pass; the feature is a working MVP observable via [command or manual check].

### Group 3: Validation and Polish

- [ ] T900 [Validation] Run acceptance validation and update validation report
  - Files: `.sdd/changes/[change-id]/validation.md`
  - Depends on: all implementation tasks
  - Spec refs: all
  - Acceptance refs: all
  - Impact seeds: none
  - No-go: none
  - TDD: no
  - Dispatch: agent (final validation selects the host high-capability provider route and resolves natively)
  - Verification: `[full local verification command]` — expected: [what output proves success]
  - Report: `.sdd/changes/[change-id]/reports/task-T900-report.md`

Checkpoint: All acceptance criteria pass or carry recorded deferrals; `validation.md` is complete.

## Dependency Notes

- [Dependency or ordering note]

## Parallel Dispatch Notes

- Tasks safe to dispatch together: [Task IDs]
- Tasks that must be serialized: [Task IDs]
- Shared files requiring controller integration: [Paths]

## Dispatch Grouping

- Routed to agents: [Task IDs] ([work class and provider-routing notes])
- Expected native by host affinity: [Task IDs] ([matching provider and host])
- Expected external worker: [Task IDs] ([non-matching provider or host rule])
- Keep inline: [Task IDs] ([why — too small to hand off])
- Frontend tasks actively dispatched: [Task IDs]

## Coverage

Every requirement and acceptance ID maps to at least one task; every task maps back. A requirement with no task or a task with no requirement is a planning defect.

| Spec / Acceptance ID | Task IDs | Notes |
| --- | --- | --- |
| SPEC-001 / AC-001.1 | T002 | [Notes] |
