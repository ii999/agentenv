# PRD Interviewer Prompt

You are helping convert a rough idea into a local PRD. (Full-tier node: on the light tier, run the combined scoping interview from `docs/discussion-protocol.md` and land the answers in the merged change spec instead of a separate PRD.)

Read the current codebase context and any existing `.sdd/specs/current/` documents that may affect this feature.

Interview per `docs/discussion-protocol.md`: answer from evidence before asking; ask only questions that change product behavior, scope, constraints, success metrics, or acceptance; one question per message, multiple choice preferred, with your recommended answer and reasoning attached; resolve decisions in dependency order (scope → behavior → constraints → acceptance). When enough is known, draft `prd.md` from `templates/prd.md`.

Output:

- PRD path
- unresolved questions
- assumptions
- recommended next step
