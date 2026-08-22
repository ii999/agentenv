# Implementation Plan: [Feature Name]

## Source Artifacts

- Change ID: [change-id]
- PRD: [path]
- Architecture: [path]
- Spec: [path or specs directory]
- Spec review: [path]

## Strategy

[Summarize the implementation strategy in 2-4 paragraphs.]

## Global Constraints

Copy exact values verbatim from PRD/spec/architecture (version floors, naming rules, limits, contracts). Every task's requirements implicitly include this section, and task reviewers check against it.

- [Constraint with exact value]
- [Constraint with exact value]

## Principles Check

Check the plan against `.sdd/memory/principles.md` and project engineering standards before phase planning. A violation is either removed or justified in Complexity Tracking.

- [ ] No principle in `.sdd/memory/principles.md` is violated, or every violation has a Complexity Tracking row.
- [ ] The structure is the simplest that satisfies the spec (no speculative layers, no future-proofing).

## Research Decisions

Resolve every unknown during planning; record the outcome. Unknowns that survive planning become explicit open questions with owners.

| Unknown | Decision | Rationale | Alternatives considered |
| --- | --- | --- | --- |
| [What was unclear] | [What was chosen] | [Why] | [What else was evaluated] |

## Complexity Tracking

One row per deviation from the simplest viable structure. The third column must name the simpler design and the observable requirement it fails. Empty is the expected state for most changes.

| Deviation | Why needed | Simpler alternative rejected because |
| --- | --- | --- |
| [e.g., new abstraction layer] | [concrete current need] | [why the direct approach fails an observable requirement] |

## Workstreams

| Workstream | Purpose | Files / areas | Depends on | Parallel safe? |
| --- | --- | --- | --- | --- |
| WS-001 | [Purpose] | [Paths] | [Dependency] | [Yes/No + reason] |

## Dependency Graph

```text
[Foundational task] -> [Phase 1 tasks] -> [Phase 2 tasks] -> [Validation]
```

## Phase Plan

### Phase 1: [Name]

- Objective: [Objective]
- Spec references: [SPEC IDs]
- Acceptance gate: [AC IDs]
- Implementation notes: [Notes]

### Phase 2: [Name]

- Objective: [Objective]
- Spec references: [SPEC IDs]
- Acceptance gate: [AC IDs]
- Implementation notes: [Notes]

## Parallelization Plan

- Parallel-safe tasks: [Task IDs or workstreams]
- Serialized tasks: [Task IDs or workstreams]
- Shared-file conflict risks: [Paths]
- Integration owner: [Controller agent]

## Verification Plan

| Gate | Command or check | Expected result | Owner |
| --- | --- | --- | --- |
| Unit / integration | [Command] | [Expected] | [Agent] |
| Acceptance | [Command/manual] | [Expected] | [Agent/User] |
| Lint/typecheck | [Command] | [Expected] | [Agent] |

## TDD Policy

TDD is exceptional (see `docs/subagent-execution.md`, "Lightweight implementation policy" — binding). Default is direct implementation plus acceptance verification, with test-after coverage where acceptance criteria, risk, or project standards justify it.

`TDD: yes` tasks (empty is the expected state for most changes) — each cites a necessity trigger and its paired test-authoring task:

- [T0xx — trigger: bug reproduction | external compatibility contract | irreversible data migration | security-sensitive boundary — tests authored in T0yy]

Test authorship for every `TDD: yes` task: the paired test task routes at orchestrator-equivalent capability (high-capability lane), its reviewed failing state is checkpointed before implementation dispatch, and the implementer treats the tests as a read-only contract.

## Rollback Plan

- [How to revert safely]

## Plan Review Checklist

- [ ] Every spec requirement has at least one implementation task.
- [ ] Every task has a verification method.
- [ ] Parallel tasks do not edit the same files.
- [ ] Every `TDD: yes` cites a necessity trigger and a paired test-authoring task; the count is minimal (zero for most changes).
- [ ] Acceptance gates are executable locally.
- [ ] No unresolved unknowns: every Research Decisions row has a decision, and no "depends on" language remains in the strategy.
- [ ] Principles Check passes or every violation has a Complexity Tracking row.
- [ ] Global Constraints carry exact values, copied verbatim from source artifacts.
