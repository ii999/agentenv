# Task Brief: [Task ID]

## Change

- Change ID: [change-id]
- Task ID: [task-id]
- Report path: [repo-relative report path]

## Task Text

[Exact task text from tasks.md]

## Impact Map

[Stamp and per-seed call-site listing from `task-brief`, or a no-seeds / none sentinel line.]

This map is a verified starting point and NOT a complete boundary. Earlier tasks may have shifted call sites. The worker MUST re-verify each seed with `git grep -n -F -e <seed> -- ':(exclude).sdd/'` before implementing (identical semantics to generation) and record differences in the report's Impact Delta section. Search domain is the tracked working tree; untracked files are not searched.

## Do Not Explore

Planning confirmed these regions are unaffected. Exploration budget must not be spent there. Touching them requires reporting BLOCKED or NEEDS_CONTEXT.

- [path]

## Relevant Source Artifacts

- PRD: [path]
- Architecture: [path]
- Spec: [path]
- Plan: [path]

## Global Constraints

[Copy the plan's Global Constraints verbatim — exact values. These bind this task in addition to its own requirements.]

## Worker Execution Boundary

You are a leaf executor. Complete this assignment directly. Do not spawn subagents, invoke `delegate-to-worker`, or otherwise delegate any part of the task. Report blockers or missing context to the orchestrator.

## Binding Requirements

[Copy only the spec and acceptance criteria needed for this task.]

## Interfaces

- Consumes: [exact names/signatures from earlier tasks this task calls]
- Produces: [exact names/signatures this task must expose for later tasks]

## Files in Scope

- [Path]

## Files Out of Scope

- [Path]

## Verification

Run:

```bash
[command]
```

Expected result:

```text
[expected result]
```

## Report Contract

Write the implementation report to the report path, resolved relative to the
root of the repository you are working in. Running in a git worktree, that is
the worktree root, so the report travels with the change it describes.

Follow `templates/implementation-report.md`: a `# ` title, then `Status:`,
`Provider:`, and `Model:` as plain header lines before the first `##` section,
then the `## Implemented`, `## Verification`, `## Files Changed`, and
`## Concerns` sections. `Status:` takes one of DONE, DONE_WITH_CONCERNS,
NEEDS_CONTEXT, or BLOCKED on a plain line, not a `## Status` heading with the
value underneath. Return only:

- The `Status: <value>` line
- Commit or changed files summary
- Verification summary
- Concerns, if any
