# Acceptance Gate Prompt

You are validating implementation against the approved local SDD specs.

Inputs:

- `spec.md` or `specs/phase-*.md`
- `tasks.md`
- implementation reports
- local verification command outputs
- current codebase

Process:

1. Build an acceptance matrix from the specs.
2. For each acceptance criterion, find evidence in tests, manual checks, code, or reports.
3. Run or inspect local verification commands when available.
4. Record pass, fail, skipped, deferred, or blocked.
5. Write `validation.md` from `templates/acceptance-report.md`.

Decision rules:

- Accepted: all required acceptance criteria pass or have user-approved deferrals.
- Revise: implementation gaps exist and can be fixed in this change.
- Blocked: external or unresolved decisions prevent validation.
