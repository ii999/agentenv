# Task Report: T006

- Change ID: 001-agent-context-cli
- Recorded: 2026-08-21T20:52:03.796361+00:00
- Source: /Users/zhaiqifeng/Dev/agent-context/.sdd/changes/001-agent-context-cli/reports/task-T006-report.md
- Status: DONE

## Worker Result (verbatim)

# Implementation Report: T006

Status: DONE
Provider: claude
Model: opus native subagent

## Implemented

Authored the Phase-3 conflict-before-resolution suite red-first, against a `run`
subcommand that does not exist yet.

### `tests/security_p3.rs`

Three table-driven tests, every invocation routed through `run_ac` so AC-019.1's
per-invocation sentinel grep covers the suite. All planted secret values come
from the `SENTINELS` constants (`SENTINEL_PLAIN` for the counting provider and
the plaintext-secret config row, `SENTINEL_NESTED` for the canary provider).

`ac_016_9_every_conflict_row_fails_before_any_provider_or_target_runs` — the
injection-plan conflict matrix. Every row asserts exit 4, that **stderr** names
both conflicting sources, that the canary file is absent (AC-016.2's canary
half: conflict detected before any provider resolution), and that the probe
report is absent (target never launched). Rows:

| Row | Sources named |
| --- | --- |
| credential vs credential, across two entries | `canary_cred`, `env_cred` |
| credential vs credential, within one entry | `canary_cred`, `env_cred` |
| credential vs inject, across two entries | `canary_cred`, `beta.inject.OPENAI_API_KEY` |
| credential vs inject, within one entry | `canary_cred`, `alpha.inject.OPENAI_API_KEY` |
| inject vs inject, across two entries | `alpha.inject.OPENAI_BASE_URL`, `beta.inject.OPENAI_BASE_URL` |

Every conflict config keeps a `command` credential inside the plan, so a
resolution ordered before the conflict check would leave the canary behind. In
the inject-vs-inject row that credential targets a variable of its own, keeping
the conflict under test purely between the two `inject` tables.

`ac_016_5_and_ac_016_8_dedup_rows_resolve_each_credential_exactly_once` — dedup
rows assert exit 0, exactly one line in the counting provider's counter file,
and the probe seeing each expected target name carrying the resolved value:

| Row | Shape |
| --- | --- |
| AC-016.5 | `credential://counting_cred` and `credential://counting_cred?as=OPENAI_API_KEY` across two entries — identical effective pairs |
| AC-016.8 | `credential://counting_cred` and `credential://counting_cred?as=LLM_API_KEY` — one credential, two targets, one resolution |
| within one entry | two fields of one entry naming the same effective pair |
| repeated `--with` | `--with alpha --with alpha` on an entry carrying both a credential and an `inject` table |

`ac_018_1_run_reports_every_phase_three_exit_code_with_a_naming_message` —
one row per exit code `run` makes reachable in Phase 3, asserting the code plus
a required stderr token: exit 1 (no `--with`; no command after `--`), exit 2 (a
suspected plaintext secret in the config), exit 3 (unknown entry), exit 4
(injection conflict), exit 4 (an `env` credential whose variable is unset —
provider-resolution failure reached through `run`), and exit 127 (a target that
cannot be executed, named in the message).

### Fixtures

- `tests/fixtures/canary_provider.sh` — `canary_provider.sh <canary-path> <value>`;
  creates the canary file, prints the value on stdout only.
- `tests/fixtures/counting_provider.sh` — `counting_provider.sh <counter-path> <value>`;
  appends one line per execution, prints the value on stdout only.

Both are mode 0755, follow T005's `#!/bin/sh` + `printf` convention, and write
nothing to stderr, so a planted sentinel can never reach an inherited channel
(SPEC-019's enforcement boundary). Each carries header comments documenting its
argv contract and its pairing with the probe helper. The suite restages both into
its temp workspace with mode 0755 rather than depending on the checked-in mode
surviving a checkout.

### `test-probe`

`tests/fixtures/bin/probe.rs` plus a `[[bin]] name = "test-probe"` entry in
`Cargo.toml`. Dependency-free (`std` only). Writes `argv\t<value>` and
`env\t<NAME>=<VALUE>` records to the file named by `TEST_PROBE_OUT`, prints `out`
on stdout and `err` on stderr, and exits with the code in `TEST_PROBE_EXIT`
(default 0). With `TEST_PROBE_OUT` unset it writes no file. An unparsable
`TEST_PROBE_EXIT` or a failed write panics rather than standing in for success.

The suite's `ProbeReport` parser keeps the environment as an ordered
`Vec<(String, String)>` — no `HashMap` anywhere in the assertion helpers — and
compares secret-valued variables without printing either side, so a failing
assertion cannot leak a sentinel into the test log.

## Verification

| Command | Result |
| --- | --- |
| `cargo test --features test-keychain --test security_p3` | RED as required — 3 failed, 0 passed, zero compile/collection errors |
| `cargo test --features test-keychain --all-targets` | pre-existing suites green; only `security_p3` fails |
| `cargo clippy --all-targets --features test-keychain -- -D warnings` | clean |
| `cargo clippy --all-targets -- -D warnings` | clean |
| `cargo fmt --check` | clean |
| `cargo build --release` | clean (the new `[[bin]]` does not disturb it) |
| `cargo build --release --features test-keychain` | still fails by design on the `compile_error!` release guard |
| `cargo test --doc --features test-keychain` | 4 passed |

### `cargo test --features test-keychain --test security_p3 2>&1 | tail -5`

```text
    ac_018_1_run_reports_every_phase_three_exit_code_with_a_naming_message

test result: FAILED. 0 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.43s

error: test failed, to rerun pass `--test security_p3`
```

Every failure is an assertion, not a compile or collection error. Two panic at
`tests/helpers/mod.rs:109` (`assert_exit`) and one at `tests/security_p3.rs:703`
(`assert_stderr_mentions`):

```text
thread 'ac_016_5_and_ac_016_8_dedup_rows_resolve_each_credential_exactly_once' panicked at tests/helpers/mod.rs:109:5:
thread 'ac_016_9_every_conflict_row_fails_before_any_provider_or_target_runs' panicked at tests/helpers/mod.rs:109:5:
thread 'ac_018_1_run_reports_every_phase_three_exit_code_with_a_naming_message' panicked at tests/security_p3.rs:703:5:
```

The cause is the one the task predicts:

```text
exit 1: run without --with: expected stderr to name "--with"
stdout:
stderr: error: unrecognized subcommand 'run'
```

### `cargo test --features test-keychain --all-targets`

```text
     Running tests/credential_p2.rs
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.26s

     Running tests/query_p1.rs
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s

     Running tests/security_p1.rs
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s

     Running tests/security_p3.rs
test result: FAILED. 0 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
```

Unit tests: 98 passed. The only failures in the workspace are the three intended
red tests in `security_p3`.

### `cargo build --release`

```text
   Compiling agent-context v0.1.0 (/Users/zhaiqifeng/Dev/agent-context)
    Finished `release` profile [optimized] target(s) in 0.15s
```

Both binaries are produced in the debug build as well: adding the explicit
`[[bin]]` does not suppress autodiscovery of `src/main.rs`
(`target/debug/agent-context` and `target/debug/test-probe` both present).

## Files Changed

| Path | Change |
| --- | --- |
| `tests/security_p3.rs` | Added — the Phase-3 conflict/dedup/exit-code suite (red). |
| `tests/fixtures/bin/probe.rs` | Added — the `test-probe` target binary. |
| `tests/fixtures/canary_provider.sh` | Added — canary command provider (mode 0755). |
| `tests/fixtures/counting_provider.sh` | Added — counting command provider (mode 0755). |
| `Cargo.toml` | Added the `[[bin]] name = "test-probe"` target. |

Nothing under `src/` was touched. `tests/helpers/mod.rs`, `tests/security_p1.rs`,
`tests/query_p1.rs`, `tests/credential_p2.rs`, and every pre-existing fixture are
unmodified.

## Concerns

- **Inject-source naming is a contract this suite fixes, not one the spec spells
  out.** SPEC-016 requires a conflict message to name both sources "(credential
  names and/or `entry.inject` keys)" without fixing a rendering. The suite
  requires the dotted `<entry>.inject.<KEY>` form, matching the dotted inject
  paths SPEC-010/`list <entry>` already produce. T007 must emit exactly that
  form; the choice is recorded in the suite's module docs.
- **AC-016.9's Windows case-variant row is not in this suite.** It needs Windows
  environment-name identity and could not be compiled or run here, so it is left
  to T007's platform coverage. The task's acceptance ref scopes T006 to the
  Unix-runnable rows.
- **The exit-1 rows are weakly discriminating today.** An unknown `run`
  subcommand already exits 1, so those two rows go red on their message token
  rather than on the code. They tighten automatically once `run` exists.
- **`[[bin]] test-probe` builds in every profile,** including `--release`. That
  is what the task specifies, and release builds stay green, but the release
  artifact directory now carries a test-only binary. Moving it behind a
  `required-features` gate would keep it out of release output at the cost of an
  extra flag on every test command.
- **The probe report format assumes single-line values.** An injected value
  containing a newline would split across records. No fixture injects one; the
  limitation is documented in the probe's module docs.

## Impact Delta

The `run_ac` seed was re-verified before writing any code with the generating
search, `git grep -n -F -e run_ac -- ':(exclude).sdd/'`: 58 matches, exactly the
count the brief records, distributed across `tests/query_p1.rs` (36),
`tests/credential_p2.rs` (10), `tests/security_p1.rs` (10), and
`tests/helpers/mod.rs` (2). No call site had shifted. `tests/security_p3.rs`
adds one further call site — a single `run_plan` wrapper that forwards to
`run_ac` with `TEST_PROBE_OUT` set, so every invocation in the new suite keeps
the helper's per-invocation sentinel assertions. `run_ac` itself was not
modified.
