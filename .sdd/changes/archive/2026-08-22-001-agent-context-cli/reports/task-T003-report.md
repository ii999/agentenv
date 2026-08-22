# Task Report: T003

- Change ID: 001-agent-context-cli
- Recorded: 2026-08-21T18:45:58.942643+00:00
- Source: /Users/zhaiqifeng/Dev/agent-context/.sdd/changes/001-agent-context-cli/reports/task-T003-report.md
- Status: DONE

## Worker Result (verbatim)

# Task Report: T003

Status: DONE
Provider: pi
Model: glm-5.3
Profile: implementation

## Summary

- Implemented the Phase-1 config core in six new modules: file location
  (SPEC-001), parsing with safe diagnostics, full SPEC-002 validation (all
  11 rules) plus SPEC-013 inject rules and the SPEC-020 sensitive-name
  traversal, profile selection (SPEC-004), the SPEC-005 path grammar with
  resolution, and shallow credential status (SPEC-012).
- All contracted interfaces were produced exactly as specified
  (`Config::load`, `Config::select_profile`, `path::Segments`/`path::resolve`,
  `shallow::Status`/`shallow_status`, `CredentialRef::parse`), with 91 inline
  unit tests covering every acceptance criterion listed for this task at the
  logic level.

## Implemented

- `src/config/mod.rs`: `Config::load` orchestration (locate -> read -> parse
  -> validate -> model build), mapping every failure to an exit-2
  `AppError::Config`. Parse errors report line/column computed from the
  `toml::de::Error` span plus its `message()` (which never contains source
  content - verified empirically); the crate's `Display` (source line with
  caret) is never forwarded. Missing file, directory-instead-of-file,
  unreadable file, and undeterminable base directory each produce a
  violation naming the path and a remedy.
- `src/config/locate.rs`: SPEC-001 priority chain (explicit file ->
  `AGENT_CONTEXT_FILE` -> absolute `XDG_CONFIG_HOME` -> `$HOME/.config` on
  Unix, `%APPDATA%` on Windows with XDG not consulted). Empty values count
  as unset; a relative `XDG_CONFIG_HOME` is ignored. Pure environment logic
  (no filesystem access) so the CLI can re-resolve deterministically for
  diagnostics.
- `src/config/model.rs`: `Config` (order-preserving `Vec<Profile>` /
  `Vec<CredentialDef>` with name lookup), `Profile` (description extracted;
  profile-level `description` is not an entry), `CredentialDef` + `Provider`
  (with `kind()` tokens for output), `CredentialRef::parse` implementing the
  strict `credential://<name>[?as=<ENV>]` grammar, and
  `Config::select_profile` (flag beats env beats default; empty flag is a
  usage error; empty env counts as unset; failures list available profiles
  and suggest `agent-context list --profiles`).
- `src/config/validate.rs`: all 11 SPEC-002 rules with full aggregation
  (version, default_profile, closed root schema, container types,
  descriptions, profile-level scalars, credential closed schema with
  provider-specific fields, reference grammar + resolution, env-name rules,
  entry-level inject shape), SPEC-013 inject validation (env-name keys,
  string values, segment grammar, no self-reference, entry-relative
  resolution, injectable-scalar types, NUL check, credential-reference
  sources directing to `inject_as`), and the SPEC-020 traversal (all profile
  table fields at any depth, tables inside arrays with `records[0].api_key`
  index notation, entry-level `inject` table excluded, ASCII
  case-insensitive exact/suffix names, `credential://`-prefixed strings
  exempt). Messages never echo field values.
- `src/path.rs`: `Segments::parse`/`render` for the SPEC-005 grammar
  (unquoted segments exclude `.`/`"`/whitespace; fully-quoted segments;
  empty/partial quoting are errors; round-trip property holds) and
  `resolve` (first segment = entry name; arrays returned whole; unknown
  paths are exit-3 errors naming the path with `list`/`get` next actions).
- `src/shallow.rs`: `Status` with `json_token` (SPEC-010 tokens) and a
  humanized `Display`; `shallow_status` computes env presence (`empty` =
  unset), constant `configured` for keychain, and command executable
  discovery without process launch: direct path (separator present) must be
  a regular executable file, otherwise PATH search; Unix execute-bit /
  Windows `.exe`-`.com` executability with `.exe`/`.com` probing for
  extension-less names on Windows.
- `src/lib.rs`: module wiring only.

## Verification

| Command or check | Result | Notes |
| --- | --- | --- |
| `cargo test --lib` | pass | 91 passed, 0 failed |
| `cargo clippy --all-targets -- -D warnings` | pass | clean |
| `cargo fmt --check` | pass | clean (kept consistent with T001's gate) |
| `cargo test --test security_p1` | expected red | 0 passed, 8 failed, all with `error: unrecognized subcommand 'validate'/'list'/'get'` - the T002 suite stays red until T004 wires the CLI; zero compilation or collection errors |
| Impact-seed re-verification (`git grep -n -F -e AppError / -e Violation -- ':(exclude).sdd/'`) | pass | matches the Impact Map exactly; see Impact Delta |

## Files Changed

| Path | Change |
| --- | --- |
| `src/lib.rs` | added `pub mod config; pub mod path; pub mod shallow;` |
| `src/config/mod.rs` | new: load orchestration, env helper, read/parse diagnostics |
| `src/config/locate.rs` | new: SPEC-001 path resolution + tests |
| `src/config/model.rs` | new: typed model, `CredentialRef`, `select_profile` + tests |
| `src/config/validate.rs` | new: SPEC-002/013/020 rules + table-driven tests incl. fixture-driven tests over the T002 fixture files |
| `src/path.rs` | new: segment grammar + resolution + tests |
| `src/shallow.rs` | new: shallow status + executable discovery + tests |

No tracked file other than `src/lib.rs` was modified. `tests/`, `src/error.rs`,
`src/main.rs`, and `Cargo.toml` are untouched.

## Acceptance Coverage

| Acceptance ID | Evidence | Status |
| --- | --- | --- |
| AC-001.1 | `locate::tests::explicit_file_wins_over_everything`, `agent_context_file_beats_platform_default`; `config::tests::load_reads_the_file_via_the_environment`, `load_uses_the_explicit_file_over_the_environment` | Covered (logic level) |
| AC-001.2 | `config::tests::load_missing_file_names_the_path` | Covered (logic level) |
| AC-001.3 | `locate::tests::missing_home_names_the_remedies`, `config::tests::load_missing_default_directory_error_names_remedies` | Covered (logic level) |
| AC-001.4 | `locate::tests::relative_xdg_config_home_is_ignored`, `empty_xdg_config_home_is_ignored` | Covered (logic level) |
| AC-002.1 | `validate::tests::description_rules` (entry missing description) | Covered (logic level) |
| AC-002.2 | `validate::tests::version_rule` (`version = 2`) | Covered (logic level) |
| AC-002.3 | `validate::tests::reference_rules` (undefined `credential://missing`) | Covered (logic level) |
| AC-002.4 | `validate::tests::three_independent_violations_are_all_reported`; `config::tests::load_reports_every_validation_violation` | Covered (logic level) |
| AC-002.6 | `validate::tests::credential_name_pattern`, `credential_closed_schema`, `credential_env_name_must_be_a_valid_env_name` | Covered (logic level) |
| AC-002.7 | `validate::tests::profile_level_scalars_are_violations` | Covered (logic level) |
| AC-002.8 | `validate::tests::container_type_rules`, `mistyped_container_aggregates_with_other_violations` | Covered (logic level) |
| AC-004.1 | `model::tests::select_profile_env_beats_default` | Covered (logic level) |
| AC-004.2 | `model::tests::select_profile_flag_beats_env_and_default` | Covered (logic level) |
| AC-004.3 | `model::tests::select_profile_unknown_name_lists_profiles`, `select_profile_with_zero_profiles` | Covered (logic level) |
| AC-005.1 | `path::tests::resolve_returns_scalars_arrays_and_tables` (quoted `server."my.field"`) | Covered (logic level) |
| AC-005.2 | `path::tests::resolve_unknown_field_is_not_found_with_next_action` | Covered (logic level) |
| AC-005.3 | `path::tests::grammar_table_invalid_paths` | Covered (logic level) |
| AC-012.3 | `shallow::tests::env_provider_status_follows_variable_presence` | Covered (logic level) |
| AC-012.4 | `model::tests::reference_grammar_rejects_malformed_forms`; `validate::tests::malformed_references_are_load_time_violations` | Covered (logic level) |
| AC-013.1 | `validate::tests::inject_rules` (string source valid) | Covered (logic level) |
| AC-013.2 | `validate::tests::inject_rules` (array + datetime sources) | Covered (logic level) |
| AC-013.3 | `validate::tests::inject_rules` (credential-reference source directs to inject_as) | Covered (logic level) |
| AC-013.4 | `validate::tests::inject_rules` (self-referential path + NUL source) | Covered (logic level) |
| AC-013.5 | `validate::tests::inject_rules` (quoted multi-segment path valid) | Covered (logic level) |
| AC-020.1 | `validate::tests::sensitive_name_matrix`; `fixture_tests::sensitive_plain_fixture_names_the_field` (against the real T002 fixture) | Covered (logic level) |
| AC-020.2 | `validate::tests::sensitive_name_matrix`; `fixture_tests::sensitive_ok_fixture_is_valid` | Covered (logic level) |
| AC-020.3 | `validate::tests::sensitive_name_matrix` (token_endpoint, use_token) | Covered (logic level) |
| AC-020.4 | `fixture_tests::sensitive_nested_fixture_names_the_full_path` | Covered (logic level) |
| AC-020.5 | `fixture_tests::sensitive_array_fixture_uses_index_notation` | Covered (logic level) |
| AC-020.6 | `fixture_tests::sensitive_upper_fixture_matches_case_insensitively` | Covered (logic level) |

CLI-level assertions for these ACs turn green in T004, as the task brief
states.

## Self-Review

- [x] Scope matches the task brief.
- [x] No unrelated files changed.
- [x] Acceptance criteria are covered.
- [x] Verification evidence is recorded.
- [x] Concerns are documented.

## Concerns

- Interpretation recorded for T004 (not a defect): to satisfy AC-002.5's
  "names the config file to edit" assertion, the CLI's error rendering for
  `AppError::Config` must name the resolved file path. Two clean options
  exist without touching this task's files: (a) call
  `config::locate::resolve_path` (`pub(crate)`, pure env logic, documented
  for this purpose) once in the CLI and pass the result as `explicit_file`
  to `Config::load`, keeping the path available for rendering; or (b) call
  it again on the error path. Validation violations deliberately carry pure
  config paths (`profiles.work.llm.api_key`), per the SPEC-002 diagnostics
  rule; load-stage failures (missing/unreadable/parse) carry the file path
  in `Violation.path`.
- Interpretation of the reference scanning scope (Definitions /
  SPEC-AS-015): `description` keys are excluded at any depth from reference
  scanning (the spec sentence is unqualified), while only the *entry-level*
  `inject` table is excluded - deeper tables named `inject` are ordinary
  data and are scanned. Both readings are pinned by
  `validate::interpretation_tests`.
- `Config::load` does not enforce the SPEC-011 Unix permission gate (bits
  subset of 0600): SPEC-011 ties that check to the `validate` command
  specifically ("all SPEC-002 core rules **plus** the Unix file-permission
  check"), and T003's spec refs exclude SPEC-011. The gate belongs to T004's
  `validate` wiring.
- With an unrecognized `provider`, the credential closed-schema check is
  skipped (the allowed field set is undeterminable); the provider violation
  itself is always reported. With a mistyped `credentials` container,
  references additionally report as undefined - accurate, since nothing can
  resolve.

## Impact Delta

Impact-seed re-verification with `git grep -n -F -- ':(exclude).sdd/'`
against the tracked working tree matched the Impact Map exactly:

- `AppError`: src/error.rs:18, src/error.rs:35, src/main.rs:4, src/main.rs:77,
  src/main.rs:82, src/main.rs:85 - unchanged; consumed as-is.
- `Violation`: src/error.rs:6, src/error.rs:11, src/error.rs:23 - unchanged;
  consumed as-is.

The new modules add call sites of both types (in `src/config/`,
`src/path.rs`), which is the intended production of this task rather than a
shift of mapped sites. No mapped call site was modified or skipped.
