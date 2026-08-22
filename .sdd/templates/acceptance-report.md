# Acceptance Validation Report: [Feature Name]

## Metadata

- Change ID: [change-id]
- Date: [YYYY-MM-DD]
- Validator: [agent/person]
- Implementation range: [git range or file summary]

## Acceptance Matrix

| Acceptance ID | Requirement | Evidence | Result | Notes |
| --- | --- | --- | --- | --- |
| AC-001.1 | SPEC-001 | [command/manual check/link] | Pass | [Notes] |

## Local Verification Commands

| Command | Result | Output summary |
| --- | --- | --- |
| `[command]` | [Pass/Fail/Skipped] | [Summary] |

## Failure Triage

| Command | Classification |
| --- | --- |
| `[command]` | [pass / new failure / pre-existing failure / fixed pre-existing failure] |

Classify every failing check against the pre-implementation baseline (`sdd.py verify --baseline`): failures introduced by this change, failures that already existed, and pre-existing failures this change fixed. Only new failures block acceptance by default.

## Manual Validation

| Scenario | Steps | Result | Notes |
| --- | --- | --- | --- |
| [Scenario] | [Steps] | [Pass/Fail] | [Notes] |

## Known Deviations

| ID | Deviation | Impact | Decision |
| --- | --- | --- | --- |
| DEV-001 | [Deviation] | [Impact] | [Fix now / defer / accept] |

## Deferred Items

| Item | Reason | Follow-up |
| --- | --- | --- |
| [Item] | [Reason] | [Task/change ID] |

## Final Decision

Decision: [Accepted / Revise / Blocked]

Rationale:

- [Rationale]
