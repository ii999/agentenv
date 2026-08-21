# Implementation Specification: agent-context-cli

Authoring rules: behavior altitude, think-like-a-tester, clarification marker cap (max 3, everything else resolved as a recorded assumption) — see `docs/authoring-discipline.md`.

## Source Artifacts

- Change ID: 001-agent-context-cli
- PRD: [path]
- Architecture: [path]
- Current specs: [paths]

## Scope

### In Scope

- [Behavior or capability]

### Out of Scope

- [Excluded behavior]

## Phase Map

Order phases by priority: Phase 1 (P1) alone must be a viable MVP, and each phase needs an independent test that assumes only earlier phases exist.

| Phase | Name | Priority | Objective | Depends on | Independent test |
| --- | --- | --- | --- | --- | --- |
| Phase 1 | [Name] | P1 (MVP) | [Objective] | [None / phase] | [How to validate this phase alone] |
| Phase 2 | [Name] | P2 | [Objective] | [Phase 1] | [How to validate on top of Phase 1] |

## Requirements

### SPEC-001: [Requirement name]

The system MUST [observable behavior].

Source trace:

- PRD: [PRD-FR-001]
- Architecture: [ARCH-001]

Acceptance criteria:

- AC-001.1: GIVEN [state], WHEN [action], THEN [observable result].
- AC-001.2: GIVEN [state], WHEN [action], THEN [observable result].

Verification:

- Automated: [test or command if appropriate]
- Manual: [manual validation if appropriate]

### SPEC-002: [Requirement name]

The system MUST [observable behavior].

Source trace:

- PRD: [PRD-FR-002]
- Architecture: [ARCH-002]

Acceptance criteria:

- AC-002.1: GIVEN [state], WHEN [action], THEN [observable result].

Verification:

- Automated: [test or command if appropriate]
- Manual: [manual validation if appropriate]

## Edge Cases

| ID | Case | Expected behavior | Verification |
| --- | --- | --- | --- |
| EDGE-001 | [Boundary or failure state] | [Expected result] | [How to verify] |

## Dependencies

| Requirement | Dependency | Reason |
| --- | --- | --- |
| SPEC-002 | SPEC-001 | [Reason] |

## Acceptance Matrix

| Acceptance ID | Requirement | Phase | Verification method | Status |
| --- | --- | --- | --- | --- |
| AC-001.1 | SPEC-001 | Phase 1 | [Command/manual] | Draft |

## Implementation Notes

[Only include constraints needed to interpret the spec. Put detailed implementation instructions in plan.md.]

## Design Notes (light tier)

Light tier only — drop this section on the full tier (architecture.md owns it there). Carry the technical decisions the change needs: chosen approach, module boundaries and interfaces touched, rejected alternatives, risks. Write `No design decisions beyond existing patterns.` when that is true.

- [Decision: choice, why, and the simpler alternative rejected because ...]

## Assumptions

Informed defaults chosen instead of raising a clarification marker. Each entry is visible and reversible; a silent guess is neither.

- SPEC-AS-001: [Default chosen] because [why this is the reasonable reading].

## Clarifications

Resolved user answers, grouped by session. After logging an entry, immediately rewrite the owning section so the spec body reflects the answer.

### Session 2026-08-21

- Q: [question] -> A: [final answer] (applied to [section])

## Open Questions

| ID | Question | Blocking? | Resolution |
| --- | --- | --- | --- |
| SPEC-Q-001 | [Question] | [Yes/No] | [Open / resolved answer] |

## Review Log (light tier)

Light tier only — drop this section on the full tier (spec-review.md owns it there). Record each review round's findings, the revisions applied, and the approval decision. `sdd.py validate` requires `Decision: Approved` here before the light-tier spec passes.

### Round 1 — 2026-08-21

- [Severity: finding and resolution]

Decision: Pending
