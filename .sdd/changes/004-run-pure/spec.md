# Implementation Specification: run-pure

Authoring rules: behavior altitude, think-like-a-tester, clarification marker cap (max 3, everything else resolved as a recorded assumption) — see `docs/authoring-discipline.md`.

## Source Artifacts

- Change ID: 004-run-pure
- PRD: merged into Scope (light tier)
- Architecture: merged into Design Notes (light tier)
- Current specs: `.sdd/specs/current/001-agent-context-cli/spec.md` (run semantics, SPEC-017/018 exit and diagnostic contracts), `.sdd/specs/current/003-project-config/spec.md` (stderr notice conventions)

## Scope

### In Scope

- An opt-in `--pure` flag on `agentenv run` that launches the target with a curated minimal environment instead of the full inherited one: a fixed platform base, plus explicitly kept variables, plus the planned injections.
- A repeatable `--keep <NAME>` option that carries named variables from the parent environment into a pure run.
- Documentation of the exact per-platform base sets, the isolation boundary, and the new flags in the README (run section and threat-model section), CLI help, and the agent skill.

### Out of Scope

- Any change to default `run` behavior when `--pure` is absent.
- Pattern or glob support in `--keep` (exact names only).
- A config-file way to make runs pure by default (per-profile or per-entry `pure` settings).
- Changes to injection planning, conflict detection, credential resolution, or provider behavior. Credential-provider subprocesses (`command` providers) keep their current full inherited environment.

## Phase Map

| Phase | Name | Priority | Objective | Depends on | Independent test |
| --- | --- | --- | --- | --- | --- |
| Phase 1 | Pure run environment | P1 (MVP) | `run --pure [--keep NAME]...` builds the child environment from base + keeps + injections only | None | Launch a probe target under `--pure` and assert the exact variable set it observes |

## Requirements

### SPEC-001: Pure environment construction

WHEN `agentenv run` is invoked with `--pure`, the child environment MUST consist of exactly three groups and nothing else:

1. Every curated-base variable name that is present in the parent environment, with its parent value.
2. Every `--keep` name that is present in the parent environment, with its parent value.
3. Every planned injection (credential references and `inject` values), exactly as planned today.

The curated base is a fixed, closed list per platform; no name reaches the child by prefix, pattern, or heuristic:

- Unix-like systems: `PATH`, `HOME`, `TMPDIR`, `TERM`, `USER`, `LOGNAME`, `SHELL`, `LANG`, `TZ`, `XDG_CONFIG_HOME`, `XDG_STATE_HOME`, `AGENTENV_FILE`, `AGENTENV_PROFILE`, `AGENTENV_NO_PROJECT`, and exactly these locale variables: `LC_ALL`, `LC_COLLATE`, `LC_CTYPE`, `LC_MESSAGES`, `LC_MONETARY`, `LC_NUMERIC`, `LC_TIME`, `LC_ADDRESS`, `LC_IDENTIFICATION`, `LC_MEASUREMENT`, `LC_NAME`, `LC_PAPER`, `LC_TELEPHONE`. Any other `LC_`-prefixed name is excluded like any unlisted name.
- Windows: `PATH`, `PATHEXT`, `SystemRoot`, `SystemDrive`, `windir`, `ComSpec`, `TEMP`, `TMP`, `USERPROFILE`, `HOMEDRIVE`, `HOMEPATH`, `APPDATA`, `LOCALAPPDATA`, `ProgramData`, `ProgramFiles`, `ProgramFiles(x86)`, `ProgramW6432`, `CommonProgramFiles`, `CommonProgramFiles(x86)`, `CommonProgramW6432`, `ALLUSERSPROFILE`, `PUBLIC`, `COMPUTERNAME`, `USERNAME`, `USERDOMAIN`, `OS`, `NUMBER_OF_PROCESSORS`, `PROCESSOR_ARCHITECTURE`, `AGENTENV_FILE`, `AGENTENV_PROFILE`, `AGENTENV_NO_PROJECT`.

Name equivalence is one rule applied uniformly to base membership, `--keep` matching, deduplication, and injection override: the platform's environment-name equivalence — case-insensitive on Windows, byte-exact on Unix (the existing `same_os_environment_name` semantics). A parent variable equivalent to a base or keep name is carried once, under the parent's own spelling and with the parent's own value; the base list's spelling never replaces the parent's. Names and values are carried as OS strings unchanged, with no lossy Unicode conversion.

Base and keep names absent from the parent environment are simply absent from the child; nothing is synthesized. Injections override a same-name (per the equivalence rule) base or kept variable, matching the precedence `run` documents today.

Under `--pure`, the target program is resolved against the child environment's `PATH` on all platforms. When the child has no `PATH`, lookup of a non-absolute target falls back to platform-defined behavior; a target that genuinely cannot be found exits 127 as documented.

Source trace:

- Scope: opt-in isolation of the launched target's environment.
- Design Notes: D-1 (curated closed base), D-4 (layering order), D-6 (nested agentenv), D-7 (network variables excluded).

Acceptance criteria:

- AC-001.1: GIVEN a parent environment containing `PATH`, `HOME`, a stray `PARENT_SECRET`, and an entry injecting `OPENAI_API_KEY`, WHEN `run --pure --with <entry> -- <probe>` launches a probe that prints its full environment, THEN the probe observes `PATH`, `HOME`, and `OPENAI_API_KEY` with correct values and does not observe `PARENT_SECRET`.
- AC-001.2: GIVEN `LC_ALL` and `LC_CTYPE` set in the parent on a Unix-like system, WHEN a pure run launches the probe, THEN both variables reach the probe unchanged.
- AC-001.3: GIVEN a curated-base name unset in the parent, WHEN a pure run launches the probe, THEN that name is absent from the probe's environment.
- AC-001.4: GIVEN an entry that injects a value under a name also present in the parent environment and covered by the base or a keep, WHEN a pure run launches the probe, THEN the probe observes the injected value.
- AC-001.5: GIVEN the same command line without `--pure`, WHEN the probe runs, THEN it observes every parent variable except names overridden by an injection, which carry the injected value (current behavior, unchanged).
- AC-001.6: GIVEN a parent variable `LC_SECRET_TOKEN` carrying a sentinel value, WHEN a pure run launches the probe, THEN the probe does not observe `LC_SECRET_TOKEN`.
- AC-001.7: GIVEN a Windows parent environment whose path variable is spelled `Path`, WHEN a pure run launches the probe with no `--keep`, THEN the probe observes the variable once, under the spelling `Path`, with the parent value.
- AC-001.8: GIVEN `AGENTENV_FILE` and `AGENTENV_PROFILE` set in the parent, WHEN a pure run launches a target that itself invokes `agentenv`, THEN the nested invocation resolves the same configuration file and profile as the parent invocation would.

Verification:

- Automated: integration tests launching a probe binary (existing test-helper pattern) that serializes its environment; assertions on exact presence and absence. AC-001.7 runs in the Windows CI job. OS-string value preservation is additionally verified by a Unix unit test at the environment-selection seam using a non-Unicode value (see EDGE-005).

### SPEC-002: Keep semantics

`--keep <NAME>` MUST be accepted any number of times, only together with `--pure`. Each name MUST satisfy the existing environment-variable-name rule (`[A-Za-z_][A-Za-z0-9_]*`, the same rule `inject_as` and `?as=` use); platform-specific names outside that grammar (such as Windows names containing parentheses) cannot be kept explicitly and are covered by the curated base only. Violations are usage errors: `--keep` without `--pure`, an invalid name, or an empty name each exit with status 1, detected during argument handling — before project discovery, before configuration loading, before any credential is resolved, and before the target launches.

A `--keep` name absent from the parent environment MUST be reported on standard error, one line per missing name, naming the variable and stating that the run continues without it. The report is written and flushed to the process's standard error before any credential is resolved and before the target is launched, ordered after the untrusted-project notice when both appear, and it appears regardless of how the run subsequently ends (success, injection conflict, resolution failure, or unlaunchable target). The report concerns inheritance only: it is emitted based on parent absence even when a planned injection supplies the same name, and it never changes the exit status.

Duplicate `--keep` names are accepted and behave as one. Keep matching uses the platform name equivalence defined in SPEC-001.

Source trace:

- Scope: explicit escape hatch for extra variables.
- Design Notes: D-2 (exact names, no globs), D-3 (missing-name report).

Acceptance criteria:

- AC-002.1: GIVEN `AWS_REGION` set in the parent, WHEN `run --pure --keep AWS_REGION --with <entry> -- <probe>` runs, THEN the probe observes `AWS_REGION` with the parent value.
- AC-002.2: GIVEN `--keep NOT_SET_ANYWHERE` with that name unset, WHEN the pure run executes, THEN standard error contains a line naming `NOT_SET_ANYWHERE` as not set, the probe does not observe it, and the run's exit status is the target's exit status.
- AC-002.3: GIVEN `--keep BAD-NAME` or `--keep A=B` or `--keep ""`, WHEN `run` is invoked, THEN it exits 1 with a usage diagnostic that names `--keep`, states the accepted name grammar, and does not launch the target.
- AC-002.4: GIVEN `--keep X` without `--pure`, WHEN `run` is invoked, THEN it exits 1 with a diagnostic stating `--keep` requires `--pure` and the target is not launched.
- AC-002.5: GIVEN `--keep AWS_REGION --keep AWS_REGION`, WHEN the pure run executes, THEN behavior is identical to a single `--keep AWS_REGION`.
- AC-002.6: GIVEN `--keep NOT_SET_ANYWHERE` together with two entries whose injections conflict on one target name, WHEN the pure run executes, THEN standard error contains the missing-keep line, the run exits 4 with the existing conflict diagnostic, and no provider is resolved.

Verification:

- Automated: integration tests over the CLI; exit-status and stderr assertions.

### SPEC-003: Existing behavior preserved

Without `--pure`, `run` MUST construct the child environment exactly as it does today, and every existing `run` contract MUST hold unchanged under `--pure`: injection-conflict detection (exit 4), credential resolution order and single-resolution semantics, usage errors (exit 1, per the accepted change-001 SPEC-018 mapping), unlaunchable targets (exit 127), and propagation of the target's exit status.

Source trace:

- Scope: out-of-scope item one; Design Notes D-4.

Acceptance criteria:

- AC-003.1: GIVEN the existing `run` test suite, WHEN the change lands, THEN every pre-existing test passes unmodified.
- AC-003.2: GIVEN two entries whose injections conflict on one target name, WHEN invoked with `--pure`, THEN `run` exits 4 with the same conflict diagnostic as a non-pure run and resolves no provider.
- AC-003.3: GIVEN a pure run whose target exits with status 7, WHEN the run completes, THEN `agentenv` exits with status 7.

Verification:

- Automated: `cargo test`; targeted pure-mode variants of conflict and exit-propagation tests.

### SPEC-004: Diagnostics stay value-free and actionable

Every diagnostic this change introduces MUST name variables only, never values, and MUST follow the accepted next-action contract (change-001 SPEC-018): name the failing thing and a corrective next action. When an invalid `--keep` argument contains `=`, the diagnostic MUST NOT reproduce the argument text at or after the first `=`; it identifies the flag and the violated grammar without echoing the token's value portion. The missing-keep report, usage errors, and help text contain no environment-variable value and no credential value, preserving the no-secret invariant. This requirement governs output written by `agentenv` itself; the launched target's own output is outside the invariant, per the documented threat-model boundary.

Source trace:

- Safety and threat model (README); 001 SPEC-018; 003's diagnostic conventions.

Acceptance criteria:

- AC-004.1: GIVEN a parent environment whose variables carry sentinel values, WHEN each new diagnostic path that does not launch a target is exercised (invalid keep name, keep-without-pure), and WHEN the missing-keep path runs with a target that prints nothing, THEN no sentinel value appears in the output `agentenv` writes on standard output or standard error.
- AC-004.2: GIVEN `--keep API_KEY=<sentinel>` as a single argument, WHEN `run` is invoked, THEN it exits 1 and the sentinel does not appear on standard output or standard error.
- AC-004.3: GIVEN the diagnostics for invalid keep, keep-without-pure, and missing keep, WHEN each is produced, THEN each names the offending flag or variable and states a corrective next action.

Verification:

- Automated: sentinel-based integration tests following `tests/project_security.rs` patterns.

### SPEC-005: Documentation and boundary statement

The README's "Running with injected values" section MUST document `--pure` and `--keep`, including the exact per-platform base sets, the layering order (base, then keeps, then injections), and the notable deliberate omissions — TLS and proxy variables (`SSL_CERT_FILE`, `SSL_CERT_DIR`, `HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY` and lowercase forms) — with a `--keep` recipe for carrying them when needed.

The README's "Safety and threat model" section MUST state that `--pure` is an environment filter for the launched target only, never a sandbox: the target still runs as the same user, in the same working directory, with inherited standard streams and file descriptors, full filesystem access including the user's configuration and platform credential store, and network access; and credential-provider subprocesses (`command` providers) keep their full inherited environment.

`agentenv run --help` MUST describe both flags. The agent skill (`skills/agentenv/SKILL.md`) MUST mention `--pure` where it describes `run`, including the filter-not-sandbox boundary.

Acceptance criteria:

- AC-005.1: GIVEN the README, WHEN reading the run section, THEN both flags, both platform base lists, the layering order, and the omitted-network-variables note with its `--keep` recipe are documented and match the implementation.
- AC-005.2: GIVEN `agentenv run --help`, WHEN printed, THEN `--pure` and `--keep <NAME>` appear with accurate one-line descriptions.
- AC-005.3: GIVEN the README threat-model section and the agent skill, WHEN read, THEN both state the filter-not-sandbox boundary and the unchanged credential-provider environment.

Verification:

- Manual: README/help/skill comparison against the implemented base-set constants.

## Edge Cases

| ID | Case | Expected behavior | Verification |
| --- | --- | --- | --- |
| EDGE-001 | Parent `PATH` unset under `--pure` | Child has no `PATH`; target lookup follows the platform-defined no-`PATH` behavior, and a target that genuinely cannot be found exits 127 as documented | Integration test using a target name absent from any default search path |
| EDGE-002 | `--keep` names a curated-base variable | Accepted; behavior identical to the base alone | Integration test |
| EDGE-003 | `--keep` names an injection target, name present in parent | Injection value wins (documented precedence); no conflict error, since `--keep` is not an injection source | Integration test |
| EDGE-004 | `--keep` names an injection target, name absent from parent | Missing-keep line appears (inheritance-only report); probe observes the injected value | Integration test |
| EDGE-005 | Variable name or value that is not valid Unicode (`OsString`) | Names and values are carried as OS strings unchanged, no lossy conversion; a base variable with a non-UTF-8 value is carried, not dropped; non-matching names are dropped like any other variable | Unix unit test at the environment-selection seam (the integration probe's lossy serialization cannot detect byte changes) |
| EDGE-006 | Windows parent has `Path` while base lists `PATH`, with a `--keep PATH` also present | Case-insensitive equivalence: carried once, parent spelling `Path`, parent value | CI Windows job |
| EDGE-007 | Target needs proxy or custom CA configuration under `--pure` | Those variables are deliberately excluded from the base; the target fails with its own network error; the README documents the `--keep` recipe | Manual (README) |

## Dependencies

| Requirement | Dependency | Reason |
| --- | --- | --- |
| SPEC-002 | SPEC-001 | Keeps only exist inside pure mode |
| SPEC-004 | SPEC-002 | The missing-keep report is the main new diagnostic |

## Acceptance Matrix

| Acceptance ID | Requirement | Phase | Verification method | Status |
| --- | --- | --- | --- | --- |
| AC-001.1 | SPEC-001 | Phase 1 | Automated | Draft |
| AC-001.2 | SPEC-001 | Phase 1 | Automated | Draft |
| AC-001.3 | SPEC-001 | Phase 1 | Automated | Draft |
| AC-001.4 | SPEC-001 | Phase 1 | Automated | Draft |
| AC-001.5 | SPEC-001 | Phase 1 | Automated | Draft |
| AC-001.6 | SPEC-001 | Phase 1 | Automated | Draft |
| AC-001.7 | SPEC-001 | Phase 1 | Automated (Windows CI) | Draft |
| AC-001.8 | SPEC-001 | Phase 1 | Automated | Draft |
| AC-002.1 | SPEC-002 | Phase 1 | Automated | Draft |
| AC-002.2 | SPEC-002 | Phase 1 | Automated | Draft |
| AC-002.3 | SPEC-002 | Phase 1 | Automated | Draft |
| AC-002.4 | SPEC-002 | Phase 1 | Automated | Draft |
| AC-002.5 | SPEC-002 | Phase 1 | Automated | Draft |
| AC-002.6 | SPEC-002 | Phase 1 | Automated | Draft |
| AC-003.1 | SPEC-003 | Phase 1 | Automated | Draft |
| AC-003.2 | SPEC-003 | Phase 1 | Automated | Draft |
| AC-003.3 | SPEC-003 | Phase 1 | Automated | Draft |
| AC-004.1 | SPEC-004 | Phase 1 | Automated | Draft |
| AC-004.2 | SPEC-004 | Phase 1 | Automated | Draft |
| AC-004.3 | SPEC-004 | Phase 1 | Automated | Draft |
| AC-005.1 | SPEC-005 | Phase 1 | Manual | Draft |
| AC-005.2 | SPEC-005 | Phase 1 | Manual | Draft |
| AC-005.3 | SPEC-005 | Phase 1 | Manual | Draft |

## Implementation Notes

- The change lives in `src/runner.rs` (environment construction) and `src/cli/mod.rs` (`RunArgs`). Name-validity reuse requires promoting a validation helper to the library's public surface (the CLI is a separate binary crate; `is_valid_env_name` in `src/config/model.rs` is currently `pub(crate)`); where validation lives determines the error type and therefore the exit status, so it must surface as `AppError::Usage` (exit 1).
- Environment-name equivalence reuses the existing `same_os_environment_name` helper for every comparison this change introduces.

## Design Notes (light tier)

- D-1 Curated closed base over injections-only: the base keeps launched tools functional (exec lookup, home, locale, temp) while still excluding everything unnamed. Injections-only was rejected as unusable without `--keep PATH` boilerplate; PATH-only was rejected because locale/home breakage produces confusing target failures unrelated to the user's intent. The base is a closed name list — no prefix rules — because prefix admission (an earlier `LC_*` draft) would auto-admit arbitrary names like `LC_SECRET_TOKEN` and defeat the audit value of a curated list. Precedent: `sudo env_reset`, `nix-shell --pure`.
- D-2 Exact `--keep` names over globs: a glob such as `AWS_*` can silently re-admit broad swaths of the environment, defeating the audit value of an explicit keep list.
- D-3 Missing keeps are reported, not fatal and not silent: a keep names intent; when the parent lacks the variable the user is told on stderr and the run proceeds, mirroring how shells treat unset variables while honoring the no-silent-failure rule. The report is flushed before credential resolution and launch because `run` replaces the process on Unix — a report routed through the normal output path would be lost.
- D-4 Layering base → keeps → injections with injections last preserves `run`'s existing documented precedence (injected values override inherited ones) and keeps conflict semantics untouched: `--keep` is inheritance, never an injection source.
- D-5 Base names absent from the parent are not synthesized: `run` is an environment filter, never a fabricator; inventing values (an empty `TMPDIR`, a guessed `PATH`) would be a silent fallback.
- D-6 The base carries `agentenv`'s own control variables (`AGENTENV_FILE`, `AGENTENV_PROFILE`, `AGENTENV_NO_PROJECT`, and the Unix config/state locations `XDG_CONFIG_HOME`/`XDG_STATE_HOME`; Windows locations `APPDATA`/`LOCALAPPDATA` are already in the base): they name locations and selections, not secrets, and dropping them would make a nested `agentenv` call inside a pure run silently resolve a different configuration or profile — a silent behavior switch on the primary agent workflow. `AGENTENV_TEST_KEYCHAIN` stays excluded as a test-only hook.
- D-7 TLS and proxy variables (`SSL_CERT_*`, `*_PROXY`) are deliberately excluded from the base: proxy URLs can embed credentials, which is exactly the class `--pure` exists to strip. The cost — network failures inside targets on proxy or custom-CA machines — is paid with documentation: the README names the omission and the `--keep` recipe.
- D-8 `--pure` is an environment filter, not a sandbox, and the documentation must say so: the target keeps the caller's uid, cwd, file descriptors, filesystem (including the user config and platform credential store), and network. Credential-provider subprocesses are out of scope and keep the full parent environment.

## Assumptions

- SPEC-AS-001: The Unix base includes `TZ` and the thirteen named locale variables because they change target behavior (time, locale) without carrying secrets; the list is fixed in this change and extensible in later ones.
- SPEC-AS-002: The missing-keep report goes to standard error, does not change the exit status, and follows the single-line diagnostic conventions established by change 003's untrusted-project notice, including its pre-dispatch write-and-flush rule.
- SPEC-AS-003: `--keep` outside `--pure` is a usage error rather than a silent no-op, per the no-silent-failure rule.
- SPEC-AS-004: The Windows base list targets what child processes need to start and find per-user data; CI's Windows job is the arbiter of sufficiency for the probe tests. `CommonProgramFiles(x86)` is included to keep the 32-bit/64-bit pairs symmetric.
- SPEC-AS-005: Nested-`agentenv` consistency (AC-001.8) is verified on Unix; the Windows base carries the same `AGENTENV_*` names, so the behavior transfers.

## Clarifications

### Session 2026-08-28

- Q: What does the pure base contain? -> A: Curated safe base (platform-critical names), not injections-only (applied to SPEC-001).
- Q: Escape hatch shape? -> A: Repeatable exact-name `--keep`, missing names reported not dropped silently (applied to SPEC-002).
- Q: Branch? -> A: `sdd/004-run-pure` (recorded in manifest git block).

## Open Questions

| ID | Question | Blocking? | Resolution |
| --- | --- | --- | --- |

## Review Log (light tier)

### Round 1 — 2026-08-28

Two independent clean-context lanes reviewed the draft: Codex `gpt-5.6-sol` (xhigh, delegate, findings REV-101..107) and Claude `opus` (xhigh, native, findings REV-201..214). Both recommended Revise. Consolidated resolutions, all applied in this revision:

- [Critical] REV-101/REV-201: new usage errors specified as exit 2, colliding with the reserved config-error status. Fixed: exit 1 throughout (SPEC-002, SPEC-003, AC-002.3/4), validation ordered during argument handling before project discovery and credential resolution.
- [Critical] REV-102: `LC_` prefix rule could auto-admit arbitrary names such as `LC_SECRET_TOKEN`. Fixed: closed explicit locale list, adversarial AC-001.6, D-1 records the rejected prefix design.
- [Important] REV-202/REV-103: missing-keep report had no emission point and would be lost across Unix `exec`, and its interaction with later failures and injections was unspecified. Fixed: pre-resolution write-and-flush rule ordered after the untrusted-project notice, inheritance-only semantics, AC-002.6, EDGE-004.
- [Important] REV-203: AC-001.5 misstated today's non-pure baseline. Fixed: reworded to exclude injection-overridden names.
- [Important] REV-204: AC-004.1 asserted over target-owned channels. Fixed: scoped to agentenv-written output using non-launching paths and a silent target.
- [Important] REV-205/REV-104: name-equivalence rules unspecified for base membership, spelling survival, and dedup. Fixed: one uniform equivalence rule, parent spelling and value survive, AC-001.7, EDGE-006.
- [Important] REV-206: keep grammar excludes parenthesised Windows names the base itself lists, and the named validation helper is crate-private. Fixed: documented POSIX-portable keep grammar with base-only coverage for platform names; Implementation Notes name the visibility promotion and the `AppError::Usage` requirement.
- [Important] REV-207: `--pure` silently dropped `AGENTENV_*`/`XDG_*`, breaking nested `agentenv` invocations. Fixed: control variables added to both bases, D-6, AC-001.8.
- [Important] REV-208: TLS/proxy omission undocumented. Fixed: D-7 records the deliberate exclusion (credential-bearing proxy URLs) and SPEC-005/EDGE-007 require the README note and `--keep` recipe.
- [Important] REV-209/REV-105: no stated isolation boundary. Fixed: SPEC-005 requires the threat-model statement (filter, not sandbox; provider subprocesses unchanged), AC-005.3, D-8.
- [Important] REV-210: EDGE-001 assumed no-`PATH` prevents lookup; platform-defined fallback exists. Fixed: child-`PATH` resolution rule in SPEC-001, EDGE-001 scoped to genuinely unfindable targets.
- [Important] REV-106: invalid `--keep A=B` diagnostics could echo a sentinel value. Fixed: SPEC-004 forbids echoing at or after the first `=`, AC-004.2.
- [Minor] REV-211: misleading missing-keep line when an injection supplies the name. Fixed: inheritance-only wording in SPEC-002 and EDGE-004.
- [Minor] REV-212: next-action contract missing. Fixed: SPEC-004 carries the 001 SPEC-018 contract, AC-004.3.
- [Minor] REV-213: `CommonProgramFiles(x86)` asymmetry. Fixed: added; SPEC-AS-004 notes it.
- [Minor] REV-214/REV-107: OS-string preservation covered names only and was untestable through the lossy probe. Fixed: values included in SPEC-001 and EDGE-005, seam-level Unix unit test required.

Decision: Pending (targeted re-check of the revisions dispatched)
