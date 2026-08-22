# Acceptance Validation Report: agent-context CLI

## Metadata

- Change ID: 001-agent-context-cli
- Date: 2026-08-22
- Validator: orchestrator (Claude, host high-capability route, native)
- Implementation range: branch `sdd/001-agent-context-cli`, T001–T008 checkpoints (crate scaffold through README)

## Acceptance Matrix

Granularity: one row per SPEC requirement; each row cites the suites or manual checks covering that requirement's AC group. All automated evidence runs under `cargo test --features test-keychain --all-targets` unless noted.

| Acceptance ID | Requirement | Evidence | Result | Notes |
| --- | --- | --- | --- | --- |
| AC-001.x | SPEC-001 config file location | unit `config::locate::tests` (9 tests) | Pass | XDG/HOME/`AGENT_CONTEXT_FILE` precedence incl. empty-var-is-unset |
| AC-002.x | SPEC-002 strict core validation | unit `config::validate` + `query_p1` validate tests | Pass | All violations aggregated in one report, exit 2 |
| AC-003.x | SPEC-003 open schema | `query_p1` (nested tables, arrays, datetime types) | Pass | |
| AC-004.x | SPEC-004 profile selection | unit `config::model::tests::select_profile_*` + `query_p1` | Pass | flag > env > `default_profile`; empty counts as unset |
| AC-005.x | SPEC-005 path grammar | unit `path` tests + `query_p1` quoted-segment tests | Pass | |
| AC-006.x | SPEC-006 `list` | `query_p1` + frozen snapshot | Pass | |
| AC-007.x | SPEC-007 `show` | `query_p1` + frozen snapshot | Pass | |
| AC-008.x | SPEC-008 `get` | `query_p1` (scalar, raw JSON stored order, non-scalar refusal) | Pass | |
| AC-009.x | SPEC-009 `find` | `query_p1` (descriptions, values, `--all-profiles`, inject exclusion) | Pass | |
| AC-010.x | SPEC-010 JSON contract | `query_p1` six byte-exact snapshot assertions | Pass | Envelope/`Field`/`Match` shapes frozen |
| AC-011.x | SPEC-011 `validate` | `query_p1` (aggregation incl. Unix permission check) | Pass | |
| AC-012.x | SPEC-012 references + shallow status | unit `credential::shallow` + `credential_p2` | Pass | Shallow status never resolves a provider |
| AC-013.x | SPEC-013 `inject` tables | `query_p1` + `run_p3`/`security_p3` | Pass | |
| AC-014.x | SPEC-014 providers | `credential_p2` (env/keychain/command, redaction, UTF-8/NUL) | Pass | Suite gated on `test-keychain` (see DEV-002) |
| AC-015.x | SPEC-015 credential subcommands | `credential_p2` incl. PTY no-echo test (AC-015.5) | Pass | `--json` rejection per SPEC-AS-031 |
| AC-016.x | SPEC-016 injection plan | `security_p3` (5-row conflict matrix, canary; 4 dedup rows) + `run_p3` (array-only non-scan) | Pass | Conflict strictly before any provider resolution, proven by canary absence |
| AC-017.x | SPEC-017 process transparency | `run_p3` (stdio bytes, exit 7 propagation, signal 15) | Pass | Unix `exec`; byte assertions |
| AC-018.x | SPEC-018 exit codes | `security_p3` 7-row exit table + suites throughout | Pass | 0/1/2/3/4/127; clap 2→1 remap |
| AC-019.x | SPEC-019 no-secret invariant | `security_p1` + per-invocation sentinel scan in every `run_ac`/`run_with_input` call across all suites | Pass | Provider-captured bytes redacted; no `Display`/`Serialize` on secret types |
| AC-020.x | SPEC-020 sensitive field names | unit validate tests (exact + suffix, nested, arrays, inject exclusion) | Pass | |
| AC-021.x | SPEC-021 presentation order | `query_p1` snapshots (stored order; `serde_json`/`toml` preserve_order) | Pass | |
| AC-022.1 | SPEC-022 README | Manual inspection (this report) | Pass | Checklist verified item-by-item; §7 six-bullet snippet verbatim; commands spot-checked against built binary |

## Local Verification Commands

| Command | Result | Output summary |
| --- | --- | --- |
| `cargo build --all-targets && cargo clippy --all-targets -- -D warnings && cargo fmt --check && ./target/debug/agent-context --version` | pass | agent-context 0.1.0 |
| `agent-context 0.1.0` | fail (exit 127) | /bin/sh: agent-context: command not found |
| `./target/debug/agent-context; echo $?` | pass | 1 |
| `1` | fail (exit 127) | /bin/sh: 1: command not found |
| `cargo test --test security_p1 2>&1 | tail -5` | pass | test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.42s |
| `cargo test --lib && cargo clippy --all-targets -- -D warnings` | pass |     Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.06s |
| `cargo test --all-targets && cargo clippy --all-targets -- -D warnings && cargo fmt --check` | pass |     Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s |
| `security_p1` | fail (exit 127) | /bin/sh: security_p1: command not found |
| `cargo test --features test-keychain --all-targets && cargo build --release 2>&1 | tail -2` | pass |     Finished `release` profile [optimized] target(s) in 0.03s |
| `cargo build --release --features test-keychain` | fail (exit 101) | error: could not compile `agent-context` (lib) due to 1 previous error |
| `cargo test --features test-keychain --test security_p3 2>&1 | tail -5` | pass | test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.88s |
| `run` | fail (exit 127) | /bin/sh: run: command not found |
| `cargo test --features test-keychain --all-targets && cargo clippy --all-targets -- -D warnings && cargo fmt --check` | pass |     Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s |
| `security_p3` | fail (exit 127) | /bin/sh: security_p3: command not found |
| `grep -c '^' README.md && grep -n 'agent-context list --json' README.md` | pass | 148:agent-context list --json |
| `python3 <package-root>/scripts/sdd.py verify 001-agent-context-cli --compare-baseline --update-validation` | fail (exit 1) | /bin/sh: package-root: No such file or directory |

## Failure Triage

| Command | Classification |
| --- | --- |
| `cargo build --all-targets && cargo clippy --all-targets -- -D warnings && cargo fmt --check && ./target/debug/agent-context --version` | fixed pre-existing failure |
| `agent-context 0.1.0` | pre-existing failure |
| `./target/debug/agent-context; echo $?` | pass |
| `1` | pre-existing failure |
| `cargo test --test security_p1 2>&1 | tail -5` | pass |
| `cargo test --lib && cargo clippy --all-targets -- -D warnings` | fixed pre-existing failure |
| `cargo test --all-targets && cargo clippy --all-targets -- -D warnings && cargo fmt --check` | fixed pre-existing failure |
| `security_p1` | pre-existing failure |
| `cargo test --features test-keychain --all-targets && cargo build --release 2>&1 | tail -2` | fixed pre-existing failure |
| `cargo build --release --features test-keychain` | pre-existing failure |
| `cargo test --features test-keychain --test security_p3 2>&1 | tail -5` | pass |
| `run` | pre-existing failure |
| `cargo test --features test-keychain --all-targets && cargo clippy --all-targets -- -D warnings && cargo fmt --check` | fixed pre-existing failure |
| `security_p3` | pre-existing failure |
| `grep -c '^' README.md && grep -n 'agent-context list --json' README.md` | fixed pre-existing failure |
| `python3 <package-root>/scripts/sdd.py verify 001-agent-context-cli --compare-baseline --update-validation` | pre-existing failure |

Validator ruling on the table above (the tool extracts verification lines from tasks.md literally, so several rows are not real checks):

- `agent-context 0.1.0`, `1`, `security_p1`, `run`, `security_p3` — parser artifacts: expected-output fragments extracted as commands. Not checks; ignore.
- `cargo build --release --features test-keychain` — the release-guard negative check: this build failing on the `compile_error!` guard IS the pass condition (verified deliberately; see Manual Validation).
- `python3 <package-root>/scripts/sdd.py verify ...` — self-referential placeholder line; the run that produced this table is its own evidence.
- Every genuine command row passes, including the previously failing unfeatured `cargo test --all-targets` chain, fixed during validation by feature-gating `tests/credential_p2.rs` (DEV-002).

No new failures remain; nothing blocks acceptance.

## Manual Validation

| Scenario | Steps | Result | Notes |
| --- | --- | --- | --- |
| macOS Keychain round-trip (real store) | Scratch config with `provider = "keychain"`, `service = "agent-context-validation"`, `account = "t900-check"`; piped `credential set`, then `credential check`; deleted the item with `security delete-generic-password` afterward | Pass | `set` stored, `check` reported available, value never printed; item removed after the check |
| Cold-start latency budget (< 100 ms) | `/usr/bin/time -p` on `./target/release/agent-context list` against the example config, 3 runs | Pass | First-ever exec 0.36 s (one-time macOS binary verification); steady-state ≤ 10 ms (below timer resolution), well within budget |
| Release-guard negative check | `cargo build --release --features test-keychain` | Pass (build fails as required) | `compile_error!` fires; test store cannot ship in release |
| README checklist (AC-022.1) | Item-by-item inspection against SPEC-022; §7 snippet diffed verbatim against design-source.md; every shown command checked against `--help` of the built binary | Pass | |
| SPEC-AS-025 risk surfacing | Windows behavior specified and code-reviewed, not machine-verified in v1 | Surfaced | Stated in README "Windows support" and reported to the user in the final change summary |

## Known Deviations

| ID | Deviation | Impact | Decision |
| --- | --- | --- | --- |
| DEV-001 | `run` CLI wiring lives in `src/cli/query_cmds.rs` rather than a new `src/cli/run.rs` | None functional; module name no longer describes its contents | Accept (review finding: the clap subcommand enum already hosts non-query commands; splitting would cost cohesion). Optional later mechanical rename to `commands.rs` |
| DEV-002 | `tests/credential_p2.rs` is feature-gated `#![cfg(feature = "test-keychain")]` (orchestrator edit during validation) | Unfeatured `cargo test` skips the credential suite; canonical gate `cargo test --features test-keychain --all-targets` runs everything | Fix applied — without the gate, an unfeatured test run wrote to the user's real macOS keychain (defect found in T900; real-store item verified as test data and deleted) |
| DEV-003 | `test-probe` `[[bin]]` builds in every profile, so a test-only helper binary lands in release output | Inert without `TEST_PROBE_OUT`; no secret or config access | Accept for v1 (a `required-features` gate would force the feature flag onto every test command); revisit if the crate is ever distributed as a package |
| DEV-004 | AC-016.9 Windows case-variant conflict row has no runnable test on this host | Covered by code review of `same_environment_name` under `cfg(windows)` | Accept under SPEC-AS-025 (Windows not machine-verified in v1) |

## Deferred Items

| Item | Reason | Follow-up |
| --- | --- | --- |
| Windows machine verification (config path, Credential Manager, spawn+wait, case-insensitive conflicts) | No Windows host in this environment; SPEC-AS-025 records the risk | Run the suite on a Windows host before any release that claims Windows support |
| Optional rename `src/cli/query_cmds.rs` → `commands.rs` | Mechanical, out of T007 scope | Standalone cleanup change |

## Final Decision

Decision: Accepted

Rationale:

- Every SPEC requirement has automated evidence green under the canonical gate, or a recorded manual check.
- The security invariants hold under adversarial-style tests: conflict-before-resolution proven by canary absence, per-invocation sentinel leak scans across all suites, provider redaction, and the release-profile guard on the test store.
- The one defect found during validation (unfeatured test run reaching the real keychain) was fixed, verified in both feature configurations, and its real-store side effect cleaned up.
- Remaining risk is confined to Windows behavior, which is specified and code-reviewed but not machine-verified (SPEC-AS-025), stated in the README and surfaced to the user.
