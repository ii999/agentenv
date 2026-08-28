# Spec Review Report: Project-Scoped Configuration

## Review Metadata

- Change ID: 003-project-config
- Review round: 1
- Reviewers: two independent clean-context lanes — Claude (opus, xhigh, native subagent) and Codex (gpt-5.6-sol, xhigh, read-only delegate worker, handoff `003-project-config--spec-review-r1-codex`)
- Date: 2026-08-28
- Inputs:
  - PRD: `.sdd/changes/003-project-config/prd.md`
  - Architecture: `.sdd/changes/003-project-config/architecture.md`
  - Spec: `.sdd/changes/003-project-config/spec.md` (as of checkpoint `sdd(003-project-config): spec draft (pre-review)`)

## Summary

Both lanes independently recommended **Revise**. The lanes converged on the same core defects: the compatibility guarantee is not verifiable with the current non-hermetic test harness; `project status` is self-contradictory in degraded states; the proposed interfaces (non-fallible `ProjectContext`, bare pin in `select_profile`, outcome without exit status) cannot represent required behaviors; exit-status assignments overload status 3; the security NFR has no owning requirement; and the JSON, field-path, notice-scope, unreadable-file, and trust-store-durability contracts are under-specified. Findings below are deduplicated across lanes; lane attribution in the Evidence column.

## Coverage Matrix

Consolidated (both lanes produced consistent matrices):

| Source ID | Covered by | Status | Notes |
| --- | --- | --- | --- |
| PRD-FR-001 | SPEC-001, SPEC-004 | Covered | |
| PRD-FR-002 | SPEC-004 | Covered | Write-command applicability unspecified (IMP-001) |
| PRD-FR-003 | SPEC-006, SPEC-007 | Partial | Degraded-state behavior undefined (CRIT-002); JSON contract incomplete (IMP-004) |
| PRD-FR-004 | SPEC-003, SPEC-005 | Covered | Unreadable-file rule missing (IMP-007) |
| PRD-FR-005 | SPEC-003, SPEC-006 | Covered | Exit-status assignments contested (IMP-002) |
| PRD-FR-006 | SPEC-002 | Partial | No AC for credential-reference-shaped strings in allowed fields (MIN-002) |
| PRD-FR-007 | SPEC-005, SPEC-009 | Partial | Not verifiable as specified (CRIT-001); EDGE-004 conflict (IMP-003) |
| PRD-FR-008 | SPEC-001 AC-001.4, SPEC-008 | Covered | Trace missing on SPEC-001 (MIN-003) |
| PRD-NFR-001 | — | **Gap** | No owning requirement or ACs (IMP-005) |
| PRD-NFR-002 | SPEC-003..006 | Partial | Status 3 overload; undefined I/O failure outcomes (IMP-002, IMP-010) |
| PRD-NFR-003 | SPEC-006 | Partial | Latency clause unfalsifiable (IMP-005) |
| PRD-NFR-004 | SPEC-009 | Partial | AC-009.1 unachievable as written (CRIT-001) |

## Findings

### Critical

| ID | Finding | Evidence | Required revision | Status |
| --- | --- | --- | --- | --- |
| CRIT-001 | Compatibility guarantee (SPEC-009/AC-009.1, AC-001.2) is unachievable: the existing harness (`tests/helpers/mod.rs`) neither pins the working directory nor preserves `HOME`, so walk-to-root discovery makes every pre-existing test depend on machine state, and EDGE-004 turns a missing state base into hard failures | Both lanes (Claude Critical; Codex P2 hermeticity) | Require hermetic discovery under test control: harness pins CWD into temp trees and/or sets `AGENTENV_NO_PROJECT`; permit mechanical harness isolation; rewrite AC-001.2 as an in-build falsifiable comparison | Fixed (REV-001, REV-002) |
| CRIT-002 | `project status` undefined and self-contradictory when the active profile cannot be resolved (no user config, nothing selectable, dangling pin): specced paths exit 2/3 in exactly the states the command exists to diagnose; AC-004.4 contradicts AC-006.2/.4 | Both lanes (Claude Critical; Codex P2 command matrix) | `status` always reports discovery/trust/pin; requirement section becomes "not checked" with a named reason; degraded states get their own exit semantics; AC-004.4 scoped to commands other than `project status` | Fixed (REV-003) |
| CRIT-003 | Proposed interfaces cannot represent required outcomes: `ProjectContext::resolve` non-fallible though corrupt trust state and trusted-unreadable must fail; command outcome carries no exit status though `status` must emit a full report with exit 5/6; notices must survive failure paths and `run`'s process replacement | Codex P1 | Make project resolution fallible; specify an outcome carrying stdout/stderr/exit status; specify notice attachment on success, failure, and pre-`exec` `run` paths; reflect in architecture | Fixed (REV-004) |

### Important

| ID | Finding | Evidence | Required revision | Status |
| --- | --- | --- | --- | --- |
| IMP-001 | Pin's effect on write commands (`set`, `unset`, `--create-profile`) unspecified though ARCH-005 routes all three `select_profile` call sites through it | Claude | Decide and spec explicitly; add a write-path AC | Fixed (REV-005): pin applies uniformly to every command that resolves a profile; AC-004.8 added |
| IMP-002 | Exit statuses overload 3 (existing meaning: unknown name) for "no project file" and "unsatisfied requirement", contradicting AC-008.2, the PRD constraint, and the architecture's single new status | Both lanes | Distinct statuses: 5 = trust-state failure, 6 = requirements unsatisfied/uncheckable; invalid file at `allow` stays 2 (existing configuration-file-error class); reconcile architecture | Fixed (REV-006) |
| IMP-003 | EDGE-004 (exit 2 when state base env unset) contradicts SPEC-005 inertness for read paths | Claude | Read side: unresolvable state base ⇒ file untrusted with notice naming the reason; write side (`allow`/`revoke`): exit 2 naming the variables | Fixed (REV-007) |
| IMP-004 | JSON contract: AC-006.6 (only JSON criterion) sat in Phase 2 while SPEC-006 (Phase 1) mandates JSON; "stable JSON" never frozen as a shape (keys, types, nullability, absent-vs-null, Phase-1 baseline) | Both lanes | Freeze the full envelope in Phase 1 (requirements section present from the start), extend additively in Phase 2; split AC-006.6; snapshot every trust state | Fixed (REV-008) |
| IMP-005 | PRD-NFR-001 has no owning spec requirement or ACs; PRD-NFR-003's latency clause ("no perceptible") is unfalsifiable | Both lanes | New SPEC-010 (no-secret invariant under a project file, sentinel/canary-verified across new commands and failure states); latency discharged by a recorded manual cold-start comparison at validation, stated as such | Fixed (REV-009) |
| IMP-006 | `fields` path grammar undefined: entry-relative vs qualified, which grammar, reserved tables, duplicates | Both lanes | Entry-relative, existing segment grammar, inject-table resolution semantics; duplicates are violations; nested-path ACs added | Fixed (REV-010) |
| IMP-007 | "Unreadable" classified as both untrusted (inert) and trusted-error (exit 2) with no disambiguation rule | Claude | Rule: approval record exists for the canonical path ⇒ trusted-unreadable (exit 2); otherwise untrusted | Fixed (REV-011) |
| IMP-008 | `AGENTENV_TRUST_FILE` introduced as a supported public surface inside a non-normative note, with no requirement, ACs, or documentation entry | Both lanes | Removed entirely; tests override the state base (`XDG_STATE_HOME`/`HOME`/`LOCALAPPDATA`) | Fixed (REV-012) |
| IMP-009 | Notice/discovery command scope and evaluation order unspecified (project subcommands, `--help`/`--version`, `init`, write commands, failure paths, `run` exec ordering, config-load prerequisites for `status`) | Both lanes (Claude Minor, Codex P2 — consolidated at the higher severity) | Command/state matrix and fixed evaluation order added to SPEC-005 | Fixed (REV-013) |
| IMP-010 | Trust-store mutation durability and concurrency unspecified: read-modify-write can lose records; interrupted write can corrupt the store, then AC-003.8 bricks every command | Codex P2 (Claude Suggestion — consolidated at the higher severity) | Atomic replacement (0600-first temp + rename); interrupted mutation leaves the previous store intact; concurrency is last-writer-wins per whole-store mutation, stated; ACs added | Fixed (REV-014) |
| IMP-011 | `select_profile(flag, env, Option<&str>)` cannot satisfy AC-004.4 (error must name the project file); bare pin loses selection source | Both lanes (Claude Minor, Codex P2 — consolidated at the higher severity) | Source-tagged pin (name + originating file path) in architecture; SPEC-004 notes the origin travels with the pin | Fixed (REV-015) |

### Minor

| ID | Finding | Evidence | Suggested revision | Status |
| --- | --- | --- | --- | --- |
| MIN-001 | AC-001.2 not locally falsifiable ("previous release semantics"); duplicates SPEC-009 | Claude | Rewritten as `--no-project` byte-identity comparison in this build | Fixed (REV-002) |
| MIN-002 | No AC rejects `credential://`-shaped strings inside allowed fields (`profile`, `reason`, `fields` members) | Both lanes | AC-002.6 added | Fixed (REV-016) |
| MIN-003 | PRD-FR-008 missing from SPEC-001 source trace | Claude | Trace added | Fixed (REV-016) |
| MIN-004 | Technology-bound phrasing: AC-007.4 names a fixture inside the criterion; Implementation Notes restate ARCH-002's store path | Claude | AC-007.4 rephrased (fixture moved to Verification); store path bullet now references ARCH-002 | Fixed (REV-016) |

### Suggestions

| ID | Suggestion | Rationale | Status |
| --- | --- | --- | --- |
| SUG-001 | Concurrent trust-store write semantics | Folded into IMP-010 | Accepted (REV-014) |
| SUG-002 | Reviewer-prompt input path: `docs/authoring-discipline.md` is a skill-package document, not a repo path; the delegate lane could not read it | Lane-infrastructure note, not a spec defect; recorded so future lane briefs inline or copy the document | Accepted (noted for round 2 brief) |

## Revisions Applied

| Revision | Artifact | Change made | Finding IDs |
| --- | --- | --- | --- |
| REV-001 | spec.md | SPEC-009 gains the hermeticity requirement (harness pins CWD / sets `AGENTENV_NO_PROJECT`; mechanical isolation changes allowed); AC-009.1 reworded to assertions-pass-with-harness-isolation | CRIT-001 |
| REV-002 | spec.md | AC-001.2 rewritten as `--no-project` byte-identity within this build | CRIT-001, MIN-001 |
| REV-003 | spec.md | SPEC-006 degraded-state rules + AC-006.7/.8; AC-004.4 scoped to commands other than `project status` | CRIT-002 |
| REV-004 | architecture.md, spec.md | Fallible `ProjectContext::resolve`; command outcome with explicit exit status; notice attachment on success/failure/pre-`exec` paths | CRIT-003 |
| REV-005 | spec.md | SPEC-004 states the pin applies to every profile-resolving command including writes; AC-004.8 | IMP-001 |
| REV-006 | spec.md, architecture.md | Exit statuses 5 and 6 defined; `allow`-on-invalid stays 2; README table update folded into SPEC-008 | IMP-002 |
| REV-007 | spec.md | EDGE-004 split into read-side (untrusted + notice) and write-side (exit 2) rules; SPEC-005 states the read-side rule | IMP-003 |
| REV-008 | spec.md | Full Phase-1 JSON envelope frozen in SPEC-006; AC-006.6a/6b split; per-state snapshots required | IMP-004 |
| REV-009 | spec.md | New SPEC-010 (security invariant); latency discharged via recorded manual measurement at validation (SPEC-AS-007) | IMP-005 |
| REV-010 | spec.md | `fields` grammar defined (entry-relative, accepted segment grammar, inject-table resolution semantics, duplicates are violations); ACs added | IMP-006 |
| REV-011 | spec.md | Unreadable-file disambiguation rule stated in SPEC-005 | IMP-007 |
| REV-012 | spec.md | `AGENTENV_TRUST_FILE` removed; tests override the state base | IMP-008 |
| REV-013 | spec.md | Command/state matrix and evaluation order in SPEC-005; `run` notice AC | IMP-009 |
| REV-014 | spec.md | Trust-store atomic-replacement and concurrency semantics + ACs in SPEC-003 | IMP-010 |
| REV-015 | architecture.md, spec.md | Source-tagged pin (`ProjectPin { name, file }`) in ARCH-005; SPEC-004 origin note | IMP-011 |
| REV-016 | spec.md | AC-002.6; SPEC-001 trace addition; AC-007.4 rephrase; store-path bullet references ARCH-002 | MIN-002..004 |

## Remaining Risks

| Risk | Impact | Accepted by | Follow-up |
| --- | --- | --- | --- |
| Startup-latency NFR discharged by manual measurement, not an automated gate | A regression could ship if the manual check is skipped | Orchestrator (recorded assumption SPEC-AS-007) | Measured cold-start comparison recorded in validation.md |
| Trust-store concurrency is last-writer-wins per whole-store mutation | Simultaneous `allow` of two different files may require one re-run | Orchestrator (documented behavior) | Documented in SPEC-003 and README |

## Round 2 — Targeted cross-provider re-check (Codex gpt-5.6-sol, xhigh, read-only; handoff `003-project-config--spec-review-r2-codex`)

Verdict: Revise. Confirmed fully resolved: CRIT-001, IMP-001, IMP-004, IMP-006, IMP-008, IMP-011. Partial resolutions traced to five residual/new findings:

| ID | Severity | Finding | Required revision | Status |
| --- | --- | --- | --- | --- |
| R2-001 | Critical (P1) | SPEC-010's blanket no-echo rule contradicts SPEC-006's required exposure of `profile_pin` and requirement `reason` — no implementation can satisfy both | No-echo scoped to diagnostics/notices; `project status` report defined as the bounded, enumerated exception; AC-010.1..3 rewritten (sentinels in forbidden/diagnostic positions vs. asserted intended exposure) | Fixed (REV-017) |
| R2-002 | Important (P2) | `project status` exit semantics incomplete/inconsistent (uncheckable-with-no-requirements; missing status 2 rows; invalid-user-config and no-selectable-profile cases without ACs) | Exhaustive first-match exit matrix in SPEC-006; AC-006.9..12 added | Fixed (REV-018) |
| R2-003 | Important (P2) | Unavailable state base unrepresentable in `status` (notice forbidden for project subcommands; frozen envelope had no such state) | `trust` value `unavailable` + `trust_reason` member added to the envelope; AC-006.11; SPEC-005 evaluation order names the per-command handling | Fixed (REV-019) |
| R2-004 | Important (P2) | `TrustStore::status(path, content)` cannot classify trusted-unreadable; facade lacked structured violations and a declared composition order | Path-only `lookup` interface; composition order canonicalize → lookup → read → compare → parse; `UntrustedReason` enum carries structured violations | Fixed (REV-020) |
| R2-005 | Important (P2) | Last-writer-wins contradicts the unconditional "never drop records" invariant | Preservation scoped to the mutation's input snapshot; concurrent-overwrite trade-off stated as documented behavior | Fixed (REV-021) |

## Round 3 — Targeted re-check, cycle 2 (Codex gpt-5.6-sol, xhigh, read-only; handoff `003-project-config--spec-review-r3-codex`)

Verdict: Revise. R2-002, R2-003, R2-005 confirmed resolved. Residual/new findings:

| ID | Severity | Finding | Required revision | Status |
| --- | --- | --- | --- | --- |
| R3-001 | Critical (P1, residual of R2-001) | SPEC-010's exception list omitted envelope members SPEC-006 requires (`version`, `requirements.profile`); AC-010.3 forbade the required non-null `version` | Exception defined by reference to the complete frozen envelope including the structural context members; AC-010.3 rewritten to plant sentinels in open-schema user-config values and assert the full envelope | Fixed (REV-022) |
| R3-002 | Important (P2, residual of R2-004) | `ProjectContext::Untrusted` could not carry the pin/requirements metadata and violations `project status` must render; no variant for unapproved-unreadable | Facade carries `ProjectFileMeta` (inert pin + requires) for parseable untrusted files; `Invalid(Vec<Violation>)` covers schema violations, unparseable TOML, and unapproved-unreadable; behavioral consumers restricted to `Trusted` | Fixed (REV-023) |
| R3-003 | Important (P2, new) | SPEC-005 specified two content reads (fingerprint-time and use-time), creating a time-of-check/time-of-use window violating exact-content trust | Single immutable byte snapshot: lookup → one read → classify → fingerprint → parse the same bytes; exit-matrix reference updated; architecture composition matches | Fixed (REV-024) |

## Round 4 — Targeted re-check, cycle 3 / final (Codex gpt-5.6-sol, xhigh, read-only; handoff `003-project-config--spec-review-r4-codex`)

Verdict: Revise. R3-001 and R3-002 confirmed resolved. One residual:

| ID | Severity | Finding | Required revision | Status |
| --- | --- | --- | --- | --- |
| R4-001 | Important (P2, residual of R3-003) | The facade offered no path for `project allow` to approve the exact bytes it validated — the caller would have to re-read the file, reintroducing the time-of-check/time-of-use window | Facade-owned `project::allow`/`revoke` operations performing discovery, single read, validation, fingerprinting, and store mutation over one snapshot; SPEC-003 binds approval to the validated bytes; AC-003.12 (fault-injection) added | Fixed (REV-025) |

## Approval Decision

Decision: Revise (round 4 revision applied) — escalated to one full two-lane round per the review-loop cap (an Important finding persisted through three targeted cycles). The full round reviews the completely revised spec and architecture with fresh clean-context Codex-provider and Claude-provider lanes.

Approval rationale:

- Convergence across cycles was monotonic (14 → 5 → 3 → 1 findings), and every finding has an applied revision; the escalation round validates the whole revised artifact rather than a diff.

Conditions:

- The full round must report no unresolved Critical/Important findings for approval.
