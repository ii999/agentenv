# Implementation Specification: run-pure

Authoring rules: behavior altitude, think-like-a-tester, clarification marker cap (max 3, everything else resolved as a recorded assumption) — see `docs/authoring-discipline.md`.

## Source Artifacts

- Change ID: 004-run-pure
- PRD: merged into Scope (light tier)
- Architecture: merged into Design Notes (light tier)
- Current specs: `.sdd/specs/current/001-agent-context-cli/` (run semantics), `.sdd/specs/current/003-project-config/spec.md` (stderr notice conventions)

## Scope

### In Scope

- An opt-in `--pure` flag on `agentenv run` that launches the target with a curated minimal environment instead of the full inherited one: a fixed platform base, plus explicitly kept variables, plus the planned injections.
- A repeatable `--keep <NAME>` option that carries named variables from the parent environment into a pure run.
- Documentation of the exact per-platform base sets and the new flags in the README and CLI help.

### Out of Scope

- Any change to default `run` behavior when `--pure` is absent.
- Pattern or glob support in `--keep` (exact names only).
- A config-file way to make runs pure by default (per-profile or per-entry `pure` settings).
- Changes to injection planning, conflict detection, credential resolution, or provider behavior.

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

The curated base is fixed per platform:

- Unix-like systems: `PATH`, `HOME`, `TMPDIR`, `TERM`, `USER`, `LOGNAME`, `SHELL`, `LANG`, `TZ`, and every variable whose name starts with `LC_`.
- Windows: `PATH`, `PATHEXT`, `SystemRoot`, `SystemDrive`, `windir`, `ComSpec`, `TEMP`, `TMP`, `USERPROFILE`, `HOMEDRIVE`, `HOMEPATH`, `APPDATA`, `LOCALAPPDATA`, `ProgramData`, `ProgramFiles`, `ProgramFiles(x86)`, `ProgramW6432`, `CommonProgramFiles`, `CommonProgramW6432`, `ALLUSERSPROFILE`, `PUBLIC`, `COMPUTERNAME`, `USERNAME`, `USERDOMAIN`, `OS`, `NUMBER_OF_PROCESSORS`, `PROCESSOR_ARCHITECTURE`.

Base and keep names absent from the parent environment are simply absent from the child; nothing is synthesized. Injections override a same-name base or kept variable using the platform's environment-name equivalence (case-insensitive on Windows), matching the precedence `run` documents today.

Source trace:

- Scope: opt-in isolation of the launched target's environment.
- Design Notes: D-1 (curated base), D-4 (layering order).

Acceptance criteria:

- AC-001.1: GIVEN a parent environment containing `PATH`, `HOME`, a stray `PARENT_SECRET`, and an entry injecting `OPENAI_API_KEY`, WHEN `run --pure --with <entry> -- <probe>` launches a probe that prints its full environment, THEN the probe observes `PATH`, `HOME`, and `OPENAI_API_KEY` with correct values and does not observe `PARENT_SECRET`.
- AC-001.2: GIVEN `LC_ALL` and `LC_CTYPE` set in the parent on a Unix-like system, WHEN a pure run launches the probe, THEN both variables reach the probe unchanged.
- AC-001.3: GIVEN a curated-base name unset in the parent, WHEN a pure run launches the probe, THEN that name is absent from the probe's environment.
- AC-001.4: GIVEN an entry that injects a value under a name also present in the parent environment and covered by the base or a keep, WHEN a pure run launches the probe, THEN the probe observes the injected value.
- AC-001.5: GIVEN the same command line without `--pure`, WHEN the probe runs, THEN it observes every parent variable (current behavior, unchanged).

Verification:

- Automated: integration tests launching a probe binary (existing test-helper pattern) that serializes its environment; assertions on exact presence and absence.

### SPEC-002: Keep semantics

`--keep <NAME>` MUST be accepted any number of times, only together with `--pure`. Each name MUST satisfy the existing environment-variable-name rule (`[A-Za-z_][A-Za-z0-9_]*`, the same rule `inject_as` and `?as=` use). Violations are usage errors: `--keep` without `--pure`, an invalid name, or an empty name each exit with status 2 before any credential is resolved and before the target launches.

A `--keep` name absent from the parent environment MUST be reported on standard error by name (one line per missing name), and the run continues with that variable absent. Duplicate `--keep` names are accepted and behave as one. On Windows, keep matching uses case-insensitive name equivalence.

Source trace:

- Scope: explicit escape hatch for extra variables.
- Design Notes: D-2 (exact names, no globs), D-3 (missing-name report).

Acceptance criteria:

- AC-002.1: GIVEN `AWS_REGION` set in the parent, WHEN `run --pure --keep AWS_REGION --with <entry> -- <probe>` runs, THEN the probe observes `AWS_REGION` with the parent value.
- AC-002.2: GIVEN `--keep NOT_SET_ANYWHERE` with that name unset, WHEN the pure run executes, THEN standard error contains a line naming `NOT_SET_ANYWHERE` as not set, the probe does not observe it, and the run's exit status is the target's exit status.
- AC-002.3: GIVEN `--keep BAD-NAME` or `--keep A=B` or `--keep ""`, WHEN `run` is invoked, THEN it exits 2 with a usage diagnostic and the target is not launched.
- AC-002.4: GIVEN `--keep X` without `--pure`, WHEN `run` is invoked, THEN it exits 2 with a diagnostic stating `--keep` requires `--pure`, and the target is not launched.
- AC-002.5: GIVEN `--keep AWS_REGION --keep AWS_REGION`, WHEN the pure run executes, THEN behavior is identical to a single `--keep AWS_REGION`.

Verification:

- Automated: integration tests over the CLI; exit-status and stderr assertions.

### SPEC-003: Existing behavior preserved

Without `--pure`, `run` MUST construct the child environment exactly as it does today, and every existing `run` contract MUST hold unchanged under `--pure`: injection-conflict detection (exit 4), credential resolution order and single-resolution semantics, usage errors (exit 2), unlaunchable targets (exit 127), and propagation of the target's exit status.

Source trace:

- Scope: out-of-scope item one; Design Notes D-4.

Acceptance criteria:

- AC-003.1: GIVEN the existing `run` test suite, WHEN the change lands, THEN every pre-existing test passes unmodified.
- AC-003.2: GIVEN two entries whose injections conflict on one target name, WHEN invoked with `--pure`, THEN `run` exits 4 with the same conflict diagnostic as a non-pure run and resolves no provider.
- AC-003.3: GIVEN a pure run whose target exits with status 7, WHEN the run completes, THEN `agentenv` exits with status 7.

Verification:

- Automated: `cargo test`; targeted pure-mode variants of conflict and exit-propagation tests.

### SPEC-004: Diagnostics stay value-free

Every diagnostic this change introduces MUST name variables only, never values: the missing `--keep` report, usage errors, and help text contain no environment-variable value and no credential value, preserving the no-secret invariant.

Source trace:

- Safety and threat model (README); 003's diagnostic conventions.

Acceptance criteria:

- AC-004.1: GIVEN a parent environment whose variables carry sentinel values, WHEN each new diagnostic path is exercised (missing keep, invalid keep, keep-without-pure), THEN no sentinel value appears on standard output or standard error.

Verification:

- Automated: sentinel-based integration test following `tests/project_security.rs` patterns.

### SPEC-005: Documentation

The README's "Running with injected values" section MUST document `--pure` and `--keep`, including the exact per-platform base sets and the layering order (base, then keeps, then injections). `agentenv run --help` MUST describe both flags. The agent skill (`skills/agentenv/SKILL.md`) MUST mention `--pure` where it describes `run`.

Acceptance criteria:

- AC-005.1: GIVEN the README, WHEN reading the run section, THEN both flags, both platform base lists, and the layering order are documented and match the implementation.
- AC-005.2: GIVEN `agentenv run --help`, WHEN printed, THEN `--pure` and `--keep <NAME>` appear with accurate one-line descriptions.

Verification:

- Manual: README/help comparison against the implemented base-set constants.

## Edge Cases

| ID | Case | Expected behavior | Verification |
| --- | --- | --- | --- |
| EDGE-001 | Parent `PATH` unset under `--pure` | Child has no `PATH`; a non-absolute target that consequently cannot launch exits 127 as documented | Integration test |
| EDGE-002 | `--keep` names a curated-base variable | Accepted; behavior identical to the base alone | Integration test |
| EDGE-003 | `--keep` names an injection target | Injection value wins (documented precedence); no conflict error, since `--keep` is not an injection source | Integration test |
| EDGE-004 | `--pure` with `--keep` on Windows where parent has `Path` and keep says `PATH` | Case-insensitive equivalence: the variable is carried once | CI Windows job |
| EDGE-005 | Variable name in parent env that is not valid Unicode (`OsString`) | Base/keep comparison operates on OS strings; non-matching names are dropped like any other variable | Unit test |

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
| AC-002.1 | SPEC-002 | Phase 1 | Automated | Draft |
| AC-002.2 | SPEC-002 | Phase 1 | Automated | Draft |
| AC-002.3 | SPEC-002 | Phase 1 | Automated | Draft |
| AC-002.4 | SPEC-002 | Phase 1 | Automated | Draft |
| AC-002.5 | SPEC-002 | Phase 1 | Automated | Draft |
| AC-003.1 | SPEC-003 | Phase 1 | Automated | Draft |
| AC-003.2 | SPEC-003 | Phase 1 | Automated | Draft |
| AC-003.3 | SPEC-003 | Phase 1 | Automated | Draft |
| AC-004.1 | SPEC-004 | Phase 1 | Automated | Draft |
| AC-005.1 | SPEC-005 | Phase 1 | Manual | Draft |
| AC-005.2 | SPEC-005 | Phase 1 | Manual | Draft |

## Implementation Notes

- The change lives in `src/runner.rs` (environment construction) and `src/cli/mod.rs` (`RunArgs`); name validity reuses `is_valid_env_name` from `src/config/model.rs`.
- Environment-name equivalence reuses the existing `same_os_environment_name` helper.

## Design Notes (light tier)

- D-1 Curated base over injections-only: the base keeps launched tools functional (exec lookup, home, locale, temp) while still excluding everything unnamed. Injections-only was rejected as unusable without `--keep PATH` boilerplate; PATH-only was rejected because locale/home breakage produces confusing target failures unrelated to the user's intent. Precedent: `sudo env_reset`, `nix-shell --pure`.
- D-2 Exact `--keep` names over globs: a glob such as `AWS_*` can silently re-admit broad swaths of the environment, defeating the audit value of an explicit keep list.
- D-3 Missing keeps are reported, not fatal and not silent: a keep names intent; when the parent lacks the variable the user is told on stderr and the run proceeds, mirroring how shells treat unset variables while honoring the no-silent-failure rule.
- D-4 Layering base → keeps → injections with injections last preserves `run`'s existing documented precedence (injected values override inherited ones) and keeps conflict semantics untouched: `--keep` is inheritance, never an injection source.
- D-5 Base names absent from the parent are not synthesized: `run` is an environment filter, never a fabricator; inventing values (an empty `TMPDIR`, a guessed `PATH`) would be a silent fallback.

## Assumptions

- SPEC-AS-001: The Unix base includes `TZ` and the full `LC_` family because they change target behavior (time, locale) without carrying secrets; the list is fixed in this change and extensible in later ones.
- SPEC-AS-002: The missing-keep report goes to standard error, does not change the exit status, and follows the single-line diagnostic conventions established by change 003's untrusted-project notice.
- SPEC-AS-003: `--keep` outside `--pure` is a usage error rather than a silent no-op, per the no-silent-failure rule.
- SPEC-AS-004: The Windows base list targets what child processes need to start and find per-user data; CI's Windows job is the arbiter of sufficiency for the probe tests.

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

- Pending.

Decision: Pending
