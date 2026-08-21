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

---

# Round 2 (full two-lane re-review of the revised spec)

## Review Metadata

- Review round: 2
- Reviewers (clean-context, round-1 findings withheld):
  - Lane A: claude / opus (native subagent), DONE_WITH_CONCERNS, REVISE
  - Lane B: codex / gpt-5.6-sol @ xhigh (delegate worker, read-only), DONE_WITH_CONCERNS, REVISE — raw result: `.claude/handoffs/001-agent-context-cli--spec-review-r2/RESULT.md`
- Date: 2026-08-22

## Summary

Round-1 resolutions held (neither lane re-raised CRIT-001..004 as unresolved). Both lanes independently converged on a new set: the `find` matching rule contradicting the design example, residual JSON-contract gaps (`validate --json`, `list <entry> --json`, recursive `Field` members), credential-reference scope in arrays/`inject`/descriptions, phase ownership of `credential list`, the unspecified keychain test seam, permission-bit predicate width, and secret-bearing parser diagnostics.

## Findings (consolidated)

### Critical

| ID | Finding | Evidence (lane) | Required revision | Status |
| --- | --- | --- | --- | --- |
| R2-CRIT-001 | `find`'s normative rule (names/descriptions only) cannot reproduce design §5.5's example output or AC-009.1; only string-value matching can | A#1 | SPEC-009 rewritten: match entry names, field names, descriptions, and string-scalar values (incl. reference strings); AC-009.1 asserts the exact design-example match set; table/array name-match output defined; empty needle = exit 1 | Fixed |
| R2-CRIT-002 | JSON contract still incomplete: `validate --json` claimed by Definitions but shapeless; `list <entry> --json` unspecified; recursive `Field` member key undeclared; error-path JSON undefined | A#2, A#3, B#1 | `validate` excluded from `--json` (SPEC-AS-014); `list <entry>`/`show` share one entry envelope; table `Field` nests members in `fields`; failing `--json` = empty stdout + text stderr | Fixed |

### Important

| ID | Finding | Evidence | Required revision | Status |
| --- | --- | --- | --- | --- |
| R2-IMP-001 | Reference recognition scope collides with arrays, `inject` values, and `description` strings | A#4, B#4 | Reference scanning scope defined in Definitions: table string fields at any depth, excluding arrays, `inject`, `description` (SPEC-AS-015); AC-012.5 + EDGE-018 | Fixed |
| R2-IMP-002 | SPEC-020 wording narrower than SPEC-012's traversal, leaving nested plaintext printable | A#5 | SPEC-020 bound to the same traversal scope; AC-020.4 nested case; credentials tables covered by closed schema (SPEC-AS-021) | Fixed |
| R2-IMP-003 | "Entry" undefined; profile-level scalar keys have no behavior | A#6 | Entry defined; profile-level non-table keys = violation (SPEC-002 rule 9, AC-002.7) | Fixed |
| R2-IMP-004 | `credential list` phase ownership contradictory | A#7, B#5 | Moved to Phase 1 with its JSON envelope (AC-015.1 → Phase 1; AC-015.2–4 stay Phase 2) | Fixed |
| R2-IMP-005 | Keychain mock-store seam unspecified; conflicts with no-mock-in-production rule | A#8 | Test-gated `test-keychain` cargo feature + env-var-selected file store, absent from release builds (SPEC-AS-019; architecture Testing Strategy) | Fixed |
| R2-IMP-006 | Credential value domain inconsistent across providers (bytes vs strings) | B#2 | Credential value domain defined (non-empty, NUL-free, UTF-8) for all providers; `set` validates input; AC-014.5 extended | Fixed |
| R2-IMP-007 | TOML non-finite floats and four datetime forms lack representations | B#3, A#10 | Scalar-to-string + JSON value encoding defined (TOML lexical forms; JSON strings for datetime/non-finite floats); EDGE-009/017 | Fixed |
| R2-IMP-008 | Injection dedup identity ambiguous (raw `?as=` vs effective target); deviation from design §6.2 unrecorded | B#6 | Identity = effective (credential, target env) pair; `inject` identity = (entry, key); AC-016.5 tests default-vs-explicit equivalence; recorded SPEC-AS-012/-018 | Fixed |
| R2-IMP-009 | Windows semantics weaken design's signal-forwarding without a recorded decision | B#7 | Recorded platform deviation SPEC-AS-013 (POSIX-scoped design clause; Windows console-group equivalent) | Fixed (recorded) |
| R2-IMP-010 | Permission predicate admits execute bits (0700, 0601 pass) | B#8 | Predicate = permission bits ⊆ 0600; AC-011.2 adds 0700 | Fixed |
| R2-IMP-011 | Parse diagnostics may echo secret-bearing source lines | B#9 | SPEC-002/SPEC-019: diagnostics carry paths + line/column, never source content; AC-019.3 malformed-TOML sentinel test | Fixed |
| R2-IMP-012 | Credential-definition validation incomplete (env `name` validity, non-empty fields, `argv[0]`, unreferenceable names, open extra fields) | B#10 | SPEC-002 rule 4 tightened (name grammar, non-empty typed fields, closed schema); AC-002.6 | Fixed |

### Minor

| ID | Finding | Evidence | Revision | Status |
| --- | --- | --- | --- | --- |
| R2-MIN-001 | AC-011.2 wording contradiction, missing exit code | A#9 | Rewritten | Fixed |
| R2-MIN-002 | Text `list` vs `list --json` recursion divergence unmarked | A#11 | Marked deliberate in SPEC-010 | Fixed |
| R2-MIN-003 | architecture rows contradict raw `get --json` deviation | A#12 | render + External Interfaces rows updated | Fixed |
| R2-MIN-004 | `find` output undefined for table/array name-matches | A#13, B#11 | Defined (path + type label); AC-009.4 | Fixed |
| R2-MIN-005 | Credential definition names unconstrained at definition site | A#14 | Folded into SPEC-002 rule 4 | Fixed |
| R2-MIN-006 | `command` shallow status undefined for relative paths with separators | A#15 | Separator ⇒ file-existence check (relative to CWD) | Fixed |
| R2-MIN-007 | AC-019.1 grep overlaps the exec/steder carve-out | A#16 | Fixture-authoring constraint added to SPEC-019/AC-019.1 | Fixed |
| R2-MIN-008 | Degenerate configs/inputs unstated (absent tables, empty needle, bad XDG value) | A#17, B#11 | EDGE-016, AC-009.5, AC-001.4 | Fixed |
| R2-MIN-009 | ARCH-005 containment claim overstated for `credential set` terminal read | A#18 | ARCH-005 amended (cli wraps into `Secret` immediately) | Fixed |
| R2-MIN-010 | keyring-core test module is `mock`, not `sample` | A#19 | Architecture Testing Strategy corrected | Fixed |
| R2-MIN-011 | Path grammar lacked unquoted-charset/empty-segment rules | B#11 | Grammar completed in SPEC-005; AC-005.3 extended | Fixed |

### Suggestions

| ID | Suggestion | Status |
| --- | --- | --- |
| R2-SUG-001 | README carries §6.1/§10 provider guidance | Accepted (SPEC-022) |
| R2-SUG-002 | State "v1 writes no log files" | Accepted (Scope, SPEC-AS-020) |
| R2-SUG-003 | `--profile` with `find --all-profiles` behavior | Accepted (EDGE-015) |
| R2-SUG-004 | 3-OS CI matrix, native-store smoke tests | Deferred post-v1; recorded in architecture Testing Strategy and validation risks |

## Revisions Applied

| Revision | Artifact | Change made | Finding IDs |
| --- | --- | --- | --- |
| REV-003 | spec.md | Full round-2 revision per the tables above | R2-CRIT-001..002, R2-IMP-001..012, R2-MIN-*, R2-SUG-1..3 |
| REV-004 | architecture.md | render/External-Interfaces raw-JSON deviation; ARCH-005 amendment; keychain test seam; keyring-core `mock`; "single native binary" | R2-MIN-003, R2-MIN-009, R2-MIN-010, R2-IMP-005 |

## Approval Decision

Decision: Revise (round 2) → round 3 full two-lane re-review (security-sensitive surface ⇒ full rounds; cap 5)

Conditions:

- Round 3 must confirm R2-CRIT/IMP resolutions with no new Critical/Important findings.
