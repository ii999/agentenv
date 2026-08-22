# Architecture Design Document: [Feature Name]

## Source Artifacts

- Change ID: [change-id]
- PRD: [path]
- Related current specs: [paths]
- Relevant code areas: [paths]

## Current State

[Describe the existing system shape, constraints, patterns, and relevant seams.]

## Goals

- [Technical goal]
- [Technical goal]

## Non-Goals

- [Technical scope exclusion]

## Proposed Architecture

[Summarize the selected architecture and how it satisfies the PRD.]

## System Context

```text
[Actor/System] -> [Interface] -> [Module] -> [Dependency]
```

## Module Boundaries and Interfaces

| Module | Interface | Responsibilities | Dependencies | Notes |
| --- | --- | --- | --- | --- |
| [Module] | [Interface] | [Responsibilities] | [Dependencies] | [Invariants/error modes/performance] |

## Data Model and State

| Entity / State | Owner | Lifecycle | Validation rules | Persistence |
| --- | --- | --- | --- | --- |
| [Entity] | [Module] | [Lifecycle] | [Rules] | [Storage] |

## External Interfaces

| Interface | Consumer | Contract | Compatibility / versioning |
| --- | --- | --- | --- |
| [API/event/CLI/UI] | [Consumer] | [Contract] | [Compatibility notes] |

## Decisions and Alternatives

### Decision ARCH-001: [Decision name]

- Decision: [Chosen approach]
- Rationale: [Why this approach]
- Alternatives considered:
  - [Alternative]: [Reason rejected]
  - [Alternative]: [Reason rejected]
- Consequences: [Operational, maintenance, product, or user impact]

## Risks and Mitigations

| Risk | Impact | Mitigation | Owner |
| --- | --- | --- | --- |
| [Risk] | [Impact] | [Mitigation] | [Owner] |

## Testing Strategy

- Acceptance gates: [How acceptance criteria will be verified]
- Automated tests: [Where tests are useful]
- Manual validation: [Where manual checks are acceptable]
- TDD seams: [Necessity triggers only — bug reproduction, external compatibility contract, irreversible migration, security-sensitive boundary; `none` is the expected value for most changes]

## Rollout, Migration, and Rollback

- Rollout: [Steps]
- Migration: [Data or operational changes]
- Rollback: [How to reverse safely]

## Open Questions

| ID | Question | Impact if unresolved | Resolution |
| --- | --- | --- | --- |
| ARCH-Q-001 | [Question] | [Impact] | [Open / resolved answer] |

## Architecture Review Checklist

- [ ] Module seams are explicit.
- [ ] Interfaces include invariants, error modes, and important constraints.
- [ ] Modules are deep: each interface is small relative to the behaviour behind it, and no module merely passes calls through (deletion test).
- [ ] Every port/seam has at least two justified adapters (typically production + test); single-adapter seams are removed or justified.
- [ ] Each module's test seam matches its dependency category (in-process / local-substitutable / remote-but-owned / true external).
- [ ] Rejected alternatives are documented.
- [ ] Risks have mitigations or accepted rationale.
- [ ] Testing strategy matches risk and acceptance criteria.
- [ ] No speculative architecture is included without a PRD or spec need.
