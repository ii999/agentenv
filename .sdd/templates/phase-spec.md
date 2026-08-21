# Phase Spec: [Phase Number] - [Phase Name]

Authoring rules: behavior altitude, think-like-a-tester, clarification marker cap — see `docs/authoring-discipline.md`.

## Source Artifacts

- Change ID: [change-id]
- Parent spec: [path]
- PRD references: [IDs]
- Architecture references: [IDs]

## Objective

- Priority: [P1 (MVP) / P2 / ...]
- Independent test: [How to validate this phase with only earlier phases in place]

[One paragraph explaining the value delivered by this phase.]

## Scope

### In Scope

- [Behavior]

### Out of Scope

- [Behavior]

## Dependencies

- [None or previous phase/spec/task dependency]

## Requirements

### [PHASE]-SPEC-001: [Requirement name]

The system MUST [observable behavior].

Acceptance criteria:

- [PHASE]-AC-001.1: GIVEN [state], WHEN [action], THEN [observable result].
- [PHASE]-AC-001.2: GIVEN [state], WHEN [action], THEN [observable result].

Verification:

- [Command, test, or manual check]

## Scenarios

### Scenario: [Scenario name]

- GIVEN [state]
- WHEN [action]
- THEN [result]
- AND [additional result]

## Failure and Edge Behavior

| Case | Expected behavior | Acceptance ID |
| --- | --- | --- |
| [Case] | [Behavior] | [AC ID] |

## Phase Exit Gate

This phase is complete when:

- [Acceptance criterion]
- [Acceptance criterion]
- [Verification evidence path or command]
