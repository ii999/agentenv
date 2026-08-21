# Spec Review Report: agent-context CLI

## Review Metadata

- Change ID: 001-agent-context-cli
- Review round: 1
- Reviewers: two independent clean-context lanes (findings not shared between lanes)
  - Lane A: claude / opus (native subagent), Status DONE_WITH_CONCERNS, Recommendation REVISE
  - Lane B: codex / gpt-5.6-sol @ xhigh (delegate worker, read-only), Status DONE_WITH_CONCERNS, Recommendation REVISE — raw result: `.claude/handoffs/001-agent-context-cli--spec-review-r1/RESULT.md`
- Date: 2026-08-22
- Inputs: prd.md, architecture.md, spec.md, design-source.md, .sdd/memory/principles.md

## Summary

Both lanes judged coverage of the design document broadly complete and the security core (shallow status, conflict-before-resolution, Secret newtype, no-shell execution) sound, but converged independently on the same weak spots: the JSON contract is incomplete and self-contradictory, the sensitive-field rule is not enforced on ordinary queries, several §6 behaviors (Windows process semantics, env precedence, reference grammar) are underspecified, and Phase 1 acceptance criteria depend on later phases. Consolidated below; Lane B severities P1/P2/P3 map to Critical/Important/Minor per its own key.

## Coverage Matrix

| Source ID | Covered by | Status | Notes |
| --- | --- | --- | --- |
| Design §3–§10, PRD-FR-001..007, PRD-NFR-001..002 | SPEC-001..020 | Covered | Both lanes confirmed full mapping |
| Design §6.2 signal forwarding (Windows) | — | Gap | IMP-001 |
| Design §5.9 per-field profile key; five JSON shapes | SPEC-010 partial | Gap | CRIT-001 |
| Design §4.2 reserved-key enforcement | SPEC-003 stated, unenforced | Gap | IMP-006 |
| Design §4.4 scalar→string conversion | — | Gap | MIN-003 |
| Design §7 agent usage protocol | — | Gap (optional in design) | SUG-003 |
| PRD-NFR-003 latency half; ARCH-002 file ordering | — | Gap | IMP-011 |

## Findings

### Critical

| ID | Finding | Evidence (lane) | Required revision | Status |
| --- | --- | --- | --- | --- |
| CRIT-001 | JSON contract incomplete and self-contradictory: `get --json` bare value carries no `version` yet SPEC-010 demands one on every top-level object; credential-ref `get --json` undefined; `get`/`list --profiles`/`find` shapes never specified though AC-010.3 locks "five shapes"; per-field `profile` key (§5.9) unplaced | A#1, A#2, B#4 | Define normative schema + example for every JSON-producing command; resolve bare-value-vs-envelope explicitly | Fixed |
| CRIT-002 | Sensitive-field rule (SPEC-020) enforced only by `validate`; `get`/`show` can print a plaintext `api_key = "sk-live-…"` | B#1 | Move SPEC-020 into load-time core validation applied before every command; `validate` still aggregates all failures | Fixed |
| CRIT-003 | Command-provider inherited stderr can carry the secret, contradicting SPEC-019's absolute wording | B#2, A#12 | Scope SPEC-019 to output agent-context itself writes; record inherited provider stderr as an explicit, documented threat-model boundary (per user decision 2026-08-21: accidental-leak protection only; capturing stderr would break interactive password-manager auth) | Fixed (scoped) |
| CRIT-004 | "No shell" invariant lacks an enforceable definition (user argv may itself name a shell) | B#3 | Define invariant as: the CLI never constructs or implies a shell invocation (direct argv exec only); user-authored argv content is not policed — the config author chooses their provider command | Fixed (defined) |

### Important

| ID | Finding | Evidence (lane) | Required revision | Status |
| --- | --- | --- | --- | --- |
| IMP-001 | Windows `run` process/signal semantics unspecified (Unix covered structurally by exec) | A#3, B#8 | Specify Windows spawn+wait, exit-code passthrough, console Ctrl-C group semantics; no explicit forwarding claimed | Fixed |
| IMP-002 | Injected vs inherited env precedence undefined; AC-016.1 untestable in dirty env | A#4, B#7 | Injections override inherited values; inherited names are never conflicts; AC probes the injected names only | Fixed |
| IMP-003 | `list`/`show` rendering depth conflicts (nested recursion, `inject` double-render, array/table text form) | A#5 | Define: `list` = entries + top-level fields (`table` leaf for sub-tables incl. `inject`); `list <entry>`/`show` recurse with dotted paths; `inject` rendered only in its `←` form in `show`; array/table text forms defined | Fixed |
| IMP-004 | Multi-error `validate` has no supporting seam (Config::load carries a single error path) | A#6 | Architecture: load returns all violations (`Vec<Violation>`); all commands report the full list on exit 2 | Fixed |
| IMP-005 | `credential set` input normalization unspecified; round-trip AC can pass with corrupted bytes | A#7, B#9 | Strip exactly one trailing `\n`/`\r\n` on the stdin path; empty value after strip = error; AC asserts exact stored bytes via mock store | Fixed |
| IMP-006 | Reserved keys declared but unenforced (`inject` of wrong type, empty, nested) | A#8, B#7 | Validation rules: `inject` non-table = exit 2; empty table valid no-op; nested `inject` inside sub-tables is ordinary data; ACs added | Fixed |
| IMP-007 | Credential reference grammar admits silent fallback (`?As=`, extra params, empty name); unknown `provider` never rejected | A#9, B#7 | Strict grammar: `credential://<name>[?as=<ENV>]`, name = `[A-Za-z0-9_-]+`; anything else = load-time exit 2; provider outside {env, keychain, command} = exit 2 | Fixed |
| IMP-008 | Phase 1 ACs depend on Phases 2–3 (exit 4/127 in AC-018.1; Secret type in AC-019.2) | A#10, B#10 | Re-scope: exit-code AC split per phase; AC-019.2 moved to Phase 2; Phase 1 shallow status defined as metadata + env/PATH checks, no resolution machinery | Fixed |
| IMP-009 | Quoted path segments are shell-hostile; escape rules undefined | B#5 | Grammar: quoted segment = `"` … `"`, any char except `"` (keys containing `"` unsupported in v1, recorded assumption); docs show single-quote shell wrapping; tests pass argv directly | Fixed |
| IMP-010 | Which commands require an active profile is undefined (breaks recovery paths) | B#6 | Per-command profile-requirement matrix added; `list --profiles`, `validate`, `credential *` never require one; `find --all-profiles` runs without one | Fixed |
| IMP-011 | File-order presentation (ARCH-002) and startup latency (PRD-NFR-003) have no acceptance gates | B#11, A#16 | Ordering AC added (text + JSON arrays follow file order); latency: structural via ARCH-001 + one-time measured budget in validation.md (manual) | Fixed |
| IMP-012 | Failure surface incomplete: unknown credential name, keychain write failure, non-UTF-8/NUL provider output, empty piped input, missing HOME, `run` with zero `--with`, `show nosuch`, `--profile` applicability | B#9, A#19 | Edge-case table and exit-code table extended; `run` requires ≥1 `--with` (usage error otherwise) | Fixed |

### Minor

| ID | Finding | Evidence | Suggested revision | Status |
| --- | --- | --- | --- | --- |
| MIN-001 | AC-009.3 permits two observable no-match behaviors | A#11 | Text mode: empty stdout + `No matches for '<needle>'` on stderr, exit 0 | Fixed |
| MIN-002 | "Query commands" never enumerated | A#13 | Enumerated: `list`, `list --profiles`, `show`, `get`, `find`, `credential list`, `validate` | Fixed |
| MIN-003 | Scalar→string conversion rules missing; inject source types broader than design §4.4 | A#14, B#7 | Conversion table added; inject sources restricted to string/integer/float/boolean (datetime excluded) | Fixed |
| MIN-004 | Command-provider contract additions (newline strip, empty-output failure) not in Assumptions | A#15 | Recorded as SPEC-AS-009 | Fixed |
| MIN-005 | Editorial residue in AC-002.1, AC-006.1, AC-007.1, AC-014.3 | A#17 | Rewritten | Fixed |
| MIN-006 | No module owns no-echo TTY input | A#18 | Architecture: `cli` row gains terminal-input responsibility (rpassword) | Fixed |

### Suggestions

| ID | Suggestion | Rationale | Status |
| --- | --- | --- | --- |
| SUG-001 | Deduplicate identical (env name, credential, `?as=`) triples across entries before conflict detection | Identical duplicates are harmless; message otherwise confusing | Accepted |
| SUG-002 | `credential list --json` | Design §5.9 uniformity; agents need the inventory | Accepted |
| SUG-003 | README documenting the §7 agent usage protocol (AGENTS.md snippet) | Adoption path is otherwise undiscoverable | Accepted (doc requirement in tasks) |

## Revisions Applied

| Revision | Artifact | Change made | Finding IDs |
| --- | --- | --- | --- |
| REV-001 | spec.md | Full revision: JSON contract section with normative shapes; SPEC-020 moved into core validation; SPEC-019 scoped with threat-model boundary; no-shell defined; Windows semantics; env precedence; rendering depth; set normalization; reserved-key + reference grammar rules; phase re-scoping; profile matrix; ordering; extended edge/error tables; editorial fixes | CRIT-001..004, IMP-001..012, MIN-001..005, SUG-001..003 |
| REV-002 | architecture.md | `Config::load` returns aggregated violations; `cli` owns no-echo terminal input (rpassword); keyring v4 / toml 1.x versions pinned from research; SPEC-019 enforcement boundary note on `credential` row | IMP-004, MIN-006 |

## Remaining Risks

| Risk | Impact | Accepted by | Follow-up |
| --- | --- | --- | --- |
| Provider process may write secrets to its own inherited stderr | Secret could reach a transcript if the provider tool itself misbehaves | User threat-model decision 2026-08-21 (accidental-leak protection only) | Documented in spec Scope and README |

## Approval Decision

Decision: Revise (round 1) → round 2 full two-lane re-review required (security-sensitive surface ⇒ full rounds, not targeted re-checks)

Approval rationale:

- Both lanes recommend REVISE; four Critical findings require spec changes before planning.

Conditions:

- Round 2 must confirm CRIT-001..004 resolutions with no new Critical/Important findings.
