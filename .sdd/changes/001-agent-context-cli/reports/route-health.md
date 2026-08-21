2026-08-21T19:12:40Z route=pi=glm-5.3 task=T004 reason=availability(provider stopReason=error, no result doc, empty worktree) -> fallback grok=grok-4.6
2026-08-21T19:32:30Z route=grok=grok-4.6 task=T004 reason=availability(malformed result, empty worktree) -> fallback codex=gpt-5.6-terra
2026-08-21T20:09:07Z task=T005 selection=codex=gpt-5.6-terra reason=route-health(pi,grok terminal failures on T004; codex=terra delivered T004)
