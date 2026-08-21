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

## Round 2 Approval Decision

Decision: Revise (round 2) → round 3 full two-lane re-review.

---

# Round 3 (full two-lane re-review)

## Review Metadata

- Review round: 3
- Reviewers (clean-context): Lane A claude/opus (native; first attempt aborted on a 504 gateway timeout, re-dispatched successfully), DONE, REVISE — no Critical. Lane B codex/gpt-5.6-sol @ xhigh (delegate, read-only), DONE_WITH_CONCERNS, REVISE — no Critical. Raw Lane B result: `.claude/handoffs/001-agent-context-cli--spec-review-r3/RESULT.md`.
- Date: 2026-08-22

## Summary

No Critical findings in either lane; both explicitly confirmed the round-2 security core holds (no-secret boundary coherent, conflict-before-resolution, shallow-status side-effect freedom, strict grammar, feasible Rust stack). Remaining Importants are frozen-contract precision issues; all revised.

## Findings (consolidated) and resolutions

Important — all Fixed in REV-005/006:

- R3-IMP-01 (A#F-1): blanket "no field values in diagnostics" contradicted the spec's own ACs and design §9 examples → diagnostics rule rewritten: never source lines or open-schema field values; closed credential-schema metadata and reference strings are citable (SPEC-002, SPEC-019).
- R3-IMP-02 (A#F-2, B#3): `Match.credential` shape undefined, status dropped; `Field.path` scope unstated → Credential summary object defined once and reused; `Match` gains `reference` + summary object; paths declared full-from-entry (SPEC-010, Definitions).
- R3-IMP-03 (A#F-3, B#3): `description` in field sets undecided → Description-as-metadata rule in Definitions: excluded from listings/`fields`/find-field-matches, addressable via `get`, included in raw `get` table output.
- R3-IMP-04 (A#F-4, B#5): AC-012.5's `run` clause was Phase-1 → split; injection halves moved to AC-016.7 (Phase 3).
- R3-IMP-05 (B#1): Windows env-name case-insensitivity → platform name identity defined (ASCII case-insensitive on Windows), spelling preserved (SPEC-016).
- R3-IMP-06 (B#2): NUL-bearing inject sources; self-referential inject paths → both load-time violations (SPEC-013, AC-013.4).
- R3-IMP-07 (B#4, A#F-6): segment grammar inconsistencies (spaces unquoted, empty quoted segments, inject-value grammar unassigned) → one segment grammar for all path-shaped inputs; whitespace excluded unquoted; empty keys unaddressable (SPEC-005, SPEC-013, SPEC-AS-024).
- R3-IMP-08 (B#6): cross-OS validation deferral unrecorded → recorded as accepted risk SPEC-AS-025 (local-first repo, no CI infra; surfaced at acceptance validation). CI matrix itself stays post-v1 — scope decision, not silently dropped.
- R3-IMP-09 (B#7): "no secret in any file" vs `credential set` store write → authorized-persistence exception stated (platform store + test-gated backing file) (SPEC-019).

Minor — all Fixed: version-required wording (A#F-8); XDG scoped to Unix (A#F-9); raw `get` table recursion/reference/description rendering (A#F-10); float text/JSON unification `1.0`, NaN sign note (A#F-11); `list` profile header (A#F-12); `--json` zero-match stderr suppression (A#F-13); PRD-NFR-003 trace notes (A#F-14, SPEC-AS-026); dangling-`default_profile` dead-path note (A#F-15); AC-017.3 signal-aware assertion (A#F-16); `show` narrowing recorded as SPEC-AS-023 (A#F-17); single-resolution-multi-target rule + AC-016.8 (A#F-18); profile names excluded from find domain (A#F-19); `credential list` description alignment (A#F-20); find reference-match shows full reference string (A#F-5); `list <entry>`/`show` divergence marked deliberate (A#F-7); unknown root keys rejected (B#P3-1, closed root schema rule 10); command shallow status = executable discovery (B#P3-2, EDGE-020).

Suggestions accepted: README sensitive-check caveat (A#F-21 → SPEC-022); JSON-alias statement (A#F-24). Noted without change: `?as=` invisibility in envelopes (A#F-22, conforms to design §5.9); 0700 rejection kept (A#F-23, SPEC-AS-006 is deliberate).

## Revisions Applied

| Revision | Artifact | Change made | Finding IDs |
| --- | --- | --- | --- |
| REV-005 | spec.md | All round-3 resolutions above | R3-IMP-01..09, R3 minors |
| REV-006 | architecture.md | none required beyond round-2 state (AS-025 note lives in spec + validation) | — |

## Round 3 Approval Decision

Decision: Revise (round 3) → round 4 full two-lane re-review.

---

# Round 4 (full two-lane re-review)

## Review Metadata

- Review round: 4
- Reviewers (clean-context): Lane A claude/opus (native), DONE_WITH_CONCERNS, REVISE — 1 Important, coverage confirmed gap-free. Lane B codex/gpt-5.6-sol @ xhigh (delegate, read-only), DONE_WITH_CONCERNS, REVISE — 1 Critical, 2 Important. Raw Lane B result: `.claude/handoffs/001-agent-context-cli--spec-review-r4/RESULT.md`.
- Date: 2026-08-22

## Findings (consolidated) and resolutions — all Fixed in REV-007/008

- R4-CRIT-01 (B#1): sensitive-name traversal inherited reference scanning's array exclusion, so `records = [{ api_key = "sk-live" }]` could bypass the guardrail → SPEC-020 traversal decoupled and extended through tables nested inside arrays (indexed display paths); AC-020.5; reference scanning unchanged.
- R4-IMP-01 (A#1): provider-captured candidate bytes from a *failed* resolution were outside the "resolved secret" wording and could legally reach exit-4 diagnostics → SPEC-019 clause added; AC-014.5 fixtures now print sentinels before failing.
- R4-IMP-02 (B#2): visible-but-unaddressable keys broke the Field.path round-trip promise → `"addressable": false` display-form marker defined in SPEC-010.
- R4-IMP-03 (B#3, A#7): mistyped core containers (`profiles = []`) had no validation contract → SPEC-002 rule 11 (generic parse first, aggregate all violations); AC-002.8.
- Minors fixed: `inject` members excluded from `find`'s match domain (A#2); `profile_description` added to the `list` envelope (A#3); `Field` gains `reference` making `?as=` visible, table-`fields` recorded as the design-"value" reading (A#4); AC-019.1 respecified as per-invocation helper assertion (A#5); dedup wording covers within-entry duplicates (A#6); Phase-1 shallow status defined as a free function, `Provider` trait arrives in Phase 2 (A#8); SPEC-020 matching made ASCII case-insensitive, AC-020.6 (A#9); `credential set <undefined>` = exit 3, run-path resolution failure added to AC-018.1 (A#10); AC-022.1 expanded to the full SPEC-022 checklist (B#4); keychain-seam description unified across architecture (B#5).
- Suggestions accepted into Implementation Notes: never forward `toml::de::Error` Display (A#12); wrap captured bytes at the capture boundary (A#15); re-verify crate features at implementation start (A#11); diagnostics cite `argv[0]` only (A#13); README notes `inject_as` discoverability (A#14).

## Round 4 Approval Decision

Decision: Revise (round 4) → round 5 full two-lane re-review (final round under the cap).

---

# Round 5 (full two-lane re-review — final under the 5-round cap)

## Review Metadata

- Review round: 5
- Reviewers (clean-context): Lane A claude/opus (native), DONE_WITH_CONCERNS, REVISE — 0 Critical, 2 Important. Lane B codex/gpt-5.6-sol @ xhigh (delegate, read-only), DONE_WITH_CONCERNS, REVISE — 1 Critical, 7 Important. Raw Lane B result: `.claude/handoffs/001-agent-context-cli--spec-review-r5/RESULT.md`.
- Date: 2026-08-22

## Summary

Both lanes' top finding was the same regression introduced by the round-4 edit: SPEC-002 rule 8 still said "same traversal scope as reference scanning" while SPEC-020 declared the broader scope (Lane B rated it Critical, Lane A Important). The remaining Importants were precision items in the frozen JSON contract, Windows discovery semantics, secret-type feasibility, and test-store isolation. All were revised (REV-009/010) after the round closed; **per the 5-round cap no further full round confirms these fixes** — see Approval Decision.

## Findings (consolidated) and resolutions — all Fixed in REV-009/010

- R5-CRIT-01 (B#1, A#1): rule 8 vs SPEC-020 scope contradiction → rule 8 now delegates to SPEC-020 as sole scope authority; the array-nested `credential://`-prefixed sensitive field question resolved via the prefix reading (A#4 merged).
- R5-IMP-01 (A#2): SPEC-020 over `inject` keys made `GITHUB_TOKEN = "path"` unfixable (SPEC-013 forbids the suggested remedy) → `inject` table excluded from sensitive traversal (keys are machinery), stated with rationale.
- R5-IMP-02 (B#2): unaddressable keys had no implementable frozen contract → exact members defined (`path: null`, `key`, `addressable: false`, ancestor propagation, unaddressable entry names reachable only via profile-level `list`).
- R5-IMP-03 (B#3): §5.9 per-field profile deviation unrecorded → SPEC-AS-027 (envelope-level profile is the approved interpretation).
- R5-IMP-04 (B#4): README claimed `inject_as` discoverable via `credential list` but the contract didn't expose it → `inject_as` added to `credential list` text + JSON; SPEC-022 wording aligned (also covers A#11's `?as=` discovery note).
- R5-IMP-05 (B#5): `Secret(String)` cannot hold pre-validation bytes → ARCH-005 rewritten as two-stage `CapturedSecret(Vec<u8>)` → checked conversion → `Secret(String)`.
- R5-IMP-06 (B#6): `PATHEXT` discovery implied shell-interpreted extensions, conflicting with no-shell execution → Windows discovery/resolution limited to direct-launch extensions (`.exe`, `.com`); scripts must name their interpreter in `argv[0]`.
- R5-IMP-07 (B#7): a cargo feature alone doesn't keep the file-backed store out of release builds → SPEC-AS-019 hardened: `all(feature, debug_assertions)` + `compile_error!` on release, negative release-artifact check at validation.
- R5-IMP-08 (B#8): coverage gaps → AC-015.5 (PTY no-echo), AC-010.4 (alias byte-equality), AC-016.9 (full conflict/dedup/platform-case matrix).
- Minors fixed: SPEC-002 argv diagnostics defer to `argv[0]`-only (A#3); `find` one-match-per-path + entry-descriptions-only (B#9); clap exit-code remap + `--help`/`--version`/EACCES/empty-env/multi-segment-arg behaviors (A#5/6/7/15); AC-011 Unix scoping (A#13); text `list` credential rows show name+status (A#12); secrets never exported into agent-context's own env (A#10); ARCH-002 order-preserving core maps + softened keyring claim (A#9, A#16); README Windows-verification statement (A#14).

## Approval Decision

Decision: **Pending user decision.** The 5-round full-fanout cap is reached. Trajectory: R1 4C+12I → R2 2C+12I → R3 0C+10I → R4 1C+3I → R5 1C+9I(A:2I) — every finding through round 5 is revised in the artifacts, but no further clean-context round has confirmed the round-5 revisions. Per loop rules, stopping and reporting to the user with options: (a) approve as-is, (b) one targeted cross-provider re-check of only the round-5 revisions, (c) a sixth full round (explicit cap override).
