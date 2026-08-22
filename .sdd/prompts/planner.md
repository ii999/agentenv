# Planner Prompt

You are creating the implementation plan and task list from approved specs.

Inputs:

- `prd.md`
- `architecture.md`
- approved `spec.md` or `specs/`
- `spec-review.md`
- `.sdd/memory/principles.md`
- relevant codebase context
- `docs/authoring-discipline.md` (plan and task authoring rules — binding)

Process:

1. Create `plan.md` from `templates/plan.md`.
2. Resolve every unknown during planning: each "depends on how X works" becomes a research action (read the code, run the experiment) recorded in Research Decisions as Decision / Rationale / Alternatives considered. A plan may not contain an unknown.
3. Run the Principles Check against `.sdd/memory/principles.md`; justify every deviation from the simplest viable structure in Complexity Tracking, naming the simpler alternative and the observable requirement it fails.
4. Copy Global Constraints verbatim with exact values — they bind every task and become the reviewer's attention lens.
5. Create `tasks.md` from `templates/tasks.md`, grouped by spec phase in priority order (Phase 1 = MVP first), each group ending with a Checkpoint.
6. Write every task for a fresh implementer with zero session context: exact file paths, exact names and signatures, exact commands with expected output, and an `Interfaces:` field (Consumes/Produces) wherever tasks share a boundary. No placeholders (see the blocklist in `docs/authoring-discipline.md`).
7. Mark `[P]` only for true parallel safety.
8. Set each task's `Dispatch:` preference (`agent` / `inline`) per the dispatch rules in `docs/subagent-execution.md`, with a reason for every `inline` assignment.
9. Fill the Coverage table: every requirement and acceptance ID maps to tasks, every task maps back.
10. Default every task to `TDD: no`. Mark `TDD: yes` only for a necessity trigger from `docs/subagent-execution.md` ("Lightweight implementation policy"), cite the trigger, and create the paired high-capability test-authoring task (own sequential ID, immediately before the implementation task, cross-referenced via `tests: T0xx`).

Self-review (bounded, before handoff): placeholder scan, coverage walk (both directions), interface-name consistency across tasks, ambiguity check; fix inline, at most 2 iterations, then state remaining issues explicitly.

Output:

- plan path,
- tasks path,
- parallelization notes,
- dispatch grouping (agent / inline, with expected native/worker resolution),
- coverage gaps found and fixed,
- first implementation task.
