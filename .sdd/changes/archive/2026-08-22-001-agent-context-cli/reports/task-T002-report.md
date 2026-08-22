# Task Report: T002

- Change ID: 001-agent-context-cli
- Recorded: 2026-08-21T18:03:52.150409+00:00
- Source: /Users/zhaiqifeng/Dev/agent-context/.sdd/changes/001-agent-context-cli/reports/task-T002-report.md
- Status: DONE

## Worker Result (verbatim)

# Result

Status: DONE
Provider: claude
Model: opus
Profile: implementation

## Summary

Authored the Phase-1 security test suite red-first: a shared invocation harness that
scrubs the child environment and checks every captured invocation for planted secrets,
seven fixture configs, and eight tests covering AC-002.5, AC-019.1, AC-019.3 and
AC-020.1 through AC-020.6. No production code was touched.

## Implemented

- `tests/helpers/mod.rs` — shared harness for this suite and the later `query_p1`,
  `credential_p2` and `security_p3` suites.
  - `SENTINELS` plus the five named constants it aggregates: distinct high-entropy
    values, none a substring of another, one per fixture that carries a suspected
    plaintext secret.
  - `Run { stdout, stderr, code }` with a `combined()` accessor.
  - `run_ac(config, envs, args)` — resolves the binary through
    `assert_cmd::Command::cargo_bin`, calls `env_clear()` and restores only `PATH`
    (plus `SYSTEMROOT` on Windows), so no `AGENT_CONTEXT_*`, `XDG_*` or `HOME` from
    the developer's environment can steer config resolution; sets
    `AGENT_CONTEXT_FILE` to `config`, applies `envs` and `args`, captures both
    channels and the exit code, then asserts no member of `SENTINELS` appears in
    stdout or stderr before returning. This satisfies AC-019.1 per invocation, so
    no aggregator test exists that `cargo test` parallelism could skip. A leak is
    reported by sentinel index and channel, never by value, keeping the secret out
    of the test log as well.
  - `Fixture` — stages a fixture config into a temp directory at mode 0600.
    Required because git cannot record that mode and SPEC-011's Unix permission
    gate (bits must be a subset of 0600) would otherwise reject every fixture,
    making the exit-0 tests unsatisfiable and the exit-2 tests pass for the wrong
    reason.
  - `assert_exit` / `assert_mentions` / `assert_omits` — assertions that quote the
    captured (already sentinel-checked) output so a red run explains itself.
- `tests/fixtures/` — `example.toml` (the design-source §4 config with `profiles.*`,
  descriptions in English, both credentials and the `inject` table),
  `sensitive_plain.toml`, `sensitive_nested.toml`, `sensitive_array.toml`,
  `sensitive_upper.toml`, `sensitive_ok.toml`, `parse_error_sentinel.toml`.
  Each sentinel is planted in exactly one fixture; the two clean fixtures carry none.
- `tests/security_p1.rs` — eight tests named after the acceptance criteria they
  carry. AC-020.4 runs both `validate` and `list` over the same fixture ("any
  command"). The parse-error test derives the expected line number from the staged
  fixture rather than hard-coding it, and asserts the diagnostic omits `api_key =`
  so a forwarded `toml::de::Error` Display cannot pass.
- `Cargo.toml` — unchanged. `assert_cmd` 2.2.2, `predicates` 3.1.4 and `tempfile` 3
  were already present as dev-dependencies; no production dependency was added.

Deviation from the brief's literal `tests/helpers.rs` path: the module lives at
`tests/helpers/mod.rs`, the conventional layout, which the dispatch explicitly
permitted. A top-level `tests/helpers.rs` would additionally be compiled as its own
integration-test target containing zero tests. The module's own doc comment records
the layout and how suites pull it in (`mod helpers;`).

## Verification

| Command or check | Result | Notes |
| --- | --- | --- |
| `cargo test --test security_p1` | 8 failed, 0 passed | Every failure is an assertion failure on the exit code; zero compilation or collection errors |
| `cargo build --all-targets` | pass | clean |
| `cargo fmt --check` | pass | clean |
| `cargo clippy --all-targets -- -D warnings` | pass | clean |

`cargo test --test security_p1 2>&1 | tail -20`:

```text
  -h, --help            Print help
  -V, --version         Print version

  left: Some(1)
 right: Some(0)


failures:
    ac_002_5_get_refuses_a_sensitive_field_instead_of_printing_it
    ac_019_3_a_parse_error_reports_its_position_without_echoing_the_line
    ac_020_1_validate_rejects_a_plaintext_secret_naming_its_path
    ac_020_2_and_ac_020_3_validate_accepts_references_and_non_secret_names
    ac_020_4_a_nested_sensitive_field_is_rejected_by_every_command
    ac_020_5_a_sensitive_field_inside_an_array_is_rejected
    ac_020_6_sensitive_names_match_case_insensitively
    the_design_example_config_loads_cleanly

test result: FAILED. 0 passed; 8 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.35s

error: test failed, to rerun pass `--test security_p1`
```

Each failure reads `expected exit N ... left: Some(1) right: Some(N)` — the binary
exits 1 from clap's unrecognized-subcommand path, exactly the expected red state.

Parser-position check (throwaway probe outside the repository, using the same `toml`
1.x version): `parse_error_sentinel.toml` fails at line 9, column 11, which is the
line the test computes. The probe also confirms `toml::de::Error`'s `Display`
reproduces the offending source line verbatim, sentinel included — the behavior
SPEC-019 forbids forwarding and which both the `api_key =` omission assertion and the
harness sentinel check now guard.

## Files Changed

| Path | Change |
| --- | --- |
| `tests/helpers/mod.rs` | Added: shared invocation harness, sentinel registry, fixture staging |
| `tests/security_p1.rs` | Added: eight red tests for the Phase-1 security guardrails |
| `tests/fixtures/example.toml` | Added: design-source §4 config, English descriptions |
| `tests/fixtures/sensitive_plain.toml` | Added: entry-level `api_key` sentinel |
| `tests/fixtures/sensitive_nested.toml` | Added: `profiles.work.llm.extra.api_key` sentinel |
| `tests/fixtures/sensitive_array.toml` | Added: `records = [{ api_key = ... }]` sentinel |
| `tests/fixtures/sensitive_upper.toml` | Added: uppercase `TOKEN` sentinel |
| `tests/fixtures/sensitive_ok.toml` | Added: credential reference, `token_endpoint`, boolean `use_token` — must validate clean |
| `tests/fixtures/parse_error_sentinel.toml` | Added: TOML syntax error on a sentinel-bearing line |

`Cargo.toml` and `src/` are unmodified.

## Acceptance Coverage

| Acceptance ID | Evidence | Status |
| --- | --- | --- |
| AC-002.5 | `ac_002_5_get_refuses_a_sensitive_field_instead_of_printing_it` | Covered |
| AC-019.1 | `run_ac` sentinel check on every captured invocation | Covered |
| AC-019.3 | `ac_019_3_a_parse_error_reports_its_position_without_echoing_the_line` | Covered |
| AC-020.1 | `ac_020_1_validate_rejects_a_plaintext_secret_naming_its_path` | Covered |
| AC-020.2 | `ac_020_2_and_ac_020_3_validate_accepts_references_and_non_secret_names` | Covered |
| AC-020.3 | same test (`token_endpoint` string, `use_token` boolean) | Covered |
| AC-020.4 | `ac_020_4_a_nested_sensitive_field_is_rejected_by_every_command` | Covered |
| AC-020.5 | `ac_020_5_a_sensitive_field_inside_an_array_is_rejected` | Covered |
| AC-020.6 | `ac_020_6_sensitive_names_match_case_insensitively` | Covered |

## Self-Review

- [x] Scope matches the task brief.
- [x] No unrelated files changed.
- [x] Acceptance criteria are covered.
- [x] Verification evidence is recorded.
- [x] Concerns are documented.

## Concerns

- T004 must load fixtures at mode 0600 and must not canonicalize the config path in a
  way that drops the `AGENT_CONTEXT_FILE` value from diagnostics: the AC-002.5 test
  asserts the message contains the path it was given.
- The parse-error test asserts the diagnostic contains the offending line number and
  the word "line" (matched case-insensitively). Message wording is otherwise
  unconstrained, but an implementation that renders a position as `9:11` with no
  "line" token would fail this test even though it satisfies AC-019.3. The assertion
  was kept because AC-019.3 requires the position to be carried and a bare number is
  too weak a signal on its own.
- `the_design_example_config_loads_cleanly` is one test beyond the listed acceptance
  set. It keeps `example.toml` — a fixture the task requires — exercised rather than
  dead, and asserts the documented example config is loadable. It is red like the
  rest.
- `predicates` stays declared as a dev-dependency per the global constraint list; this
  suite asserts through the harness and does not use it. Later suites may.
- `Fixture` copies each fixture per test, so a suite of N tests does N copies. The
  cost is negligible and it keeps tests independent under `cargo test` parallelism.

## Impact Delta

None. The Impact Map declared no seeds, and no existing call site was found or
touched: the change is additive under `tests/`.
