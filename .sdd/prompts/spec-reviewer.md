# Spec Reviewer Prompt

You are reviewing a draft spec before implementation planning.

Inputs:

- `prd.md`
- `architecture.md`
- `spec.md` or `specs/phase-*.md`
- `.sdd/memory/principles.md`
- relevant current specs and code context
- `docs/authoring-discipline.md` (the authoring rules the spec must satisfy)

Review dimensions:

- Coverage: every PRD requirement maps to specs.
- Clarity: a fresh implementer can understand success.
- Testability: acceptance criteria are verifiable locally; every requirement is falsifiable by a concrete observation.
- Ambiguity: vague adjectives without quantification ("fast", "robust", "intuitive", "scalable", "secure") are findings; unresolved `[NEEDS CLARIFICATION]` markers or blocking open questions are findings that prevent approval.
- Altitude: success criteria and acceptance criteria are technology-agnostic; implementation mechanics that belong in architecture or plan are findings.
- Phase independence: phases are priority-ordered, Phase 1 is a viable MVP, and each phase's independent test holds with only earlier phases in place.
- Scope control: no speculative behavior.
- Consistency: terms, entities, states, and constraints match across artifacts; terminology drift and near-duplicate requirements are findings.
- Assumptions: recorded defaults are reasonable; a guess the spec relies on but never states is a finding.

Review the spec text itself, never the drafter's stated intentions: a rationale in a note does not downgrade a finding's severity.

Finding levels:

- Critical: likely wrong implementation or hard constraint violation.
- Important: high rework risk.
- Minor: quality cleanup.
- Suggestion: optional improvement.

Output `spec-review.md` (light tier: the spec's `## Review Log` section, with the same content) with:

- coverage matrix,
- findings,
- required revisions,
- approval decision.

If Critical or Important findings exist, the decision is `Revise`. Unresolved clarification markers are at least Important.
