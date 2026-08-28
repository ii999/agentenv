# Acceptance Validation Report: Project-Scoped Configuration

## Metadata

- Change ID: 003-project-config
- Date: 2026-08-28
- Validator: T900 validation agent
- Implementation range: `3cd129b..85cc727` on branch `sdd/003-project-config` (51 files changed, 3837 insertions, 78 deletions across `src/`, `tests/`, `README.md`, `skills/agentenv/SKILL.md`, `Cargo.toml`)

Evidence notation: automated test evidence names the file and the test function. Manual evidence names a scenario in the Manual Validation table below. `src/project/locate.rs` and `src/project/trust.rs` tests are in-file unit tests reached by `cargo test`.

## Acceptance Matrix

| Acceptance ID | Requirement | Evidence | Result | Notes |
| --- | --- | --- | --- | --- |
| AC-001.1 | SPEC-001 | `src/project/locate.rs::tests::discovers_project_file_in_an_ancestor`; M-01 | Pass | Commands run from `repo/a/b/c` discover the root file. |
| AC-001.2 | SPEC-001 | `src/project/locate.rs::tests::skips_directory_named_like_project_file`, `::skips_dangling_symlink_named_like_project_file` | Pass | Symlink case is Unix-gated; the walk continues past both. |
| AC-001.3 | SPEC-001 | `src/project/locate.rs::tests::nearest_project_file_wins` | Pass | Nearest file wins; no merging. |
| AC-001.4 | SPEC-001 | `tests/project_notice.rs::invalid_untrusted_files_remain_inert_and_bypass_suppresses_discovery`; M-10 | Pass | `--no-project` and `AGENTENV_NO_PROJECT=1` both leave stderr empty. Empty value counts as unset (EDGE-003, M-10). |
| AC-001.5 | SPEC-001 | M-11 | Pass | `AGENTENV_NO_PROJECT=1 agentenv project status` still discovers the file and exits 5. No automated test; verified manually. |
| AC-002.1 | SPEC-002 | `tests/project_schema.rs::rejects_every_closed_schema_violation_class` (`unknown_top_level.toml`) | Pass | Names `inject`; the fixture's sentinel value is asserted absent. |
| AC-002.2 | SPEC-002 | `tests/project_schema.rs::rejects_every_closed_schema_violation_class` (`missing_version.toml`, `wrong_version.toml`) | Pass | Both name `version`. Zero-byte file is the missing-version case (EDGE-002). |
| AC-002.3 | SPEC-002 | `tests/project_schema.rs::rejects_every_closed_schema_violation_class` (`bad_profile.toml`, `profile_not_string.toml`) | Pass | Both name `profile`. |
| AC-002.4 | SPEC-002 | `tests/project_schema.rs::rejects_every_closed_schema_violation_class` (`bad_requirement`, `empty_reason`, `empty_fields`, `bad_fields`, `duplicate_fields`, `bad_entry_key`) | Pass | Paths include indexed members (`requires.llm.fields[1]`) and the empty entry key (`requires.""`). |
| AC-002.5 | SPEC-002 | `tests/project_schema.rs::accepts_a_well_formed_project_file_in_declaration_order` | Pass | Declaration order preserved across two `[requires.*]` tables. |
| AC-002.6 | SPEC-002 | `tests/project_schema.rs::rejects_every_closed_schema_violation_class` (`credential_reference`, `credential_reason`, `credential_field`) | Pass | Each asserts `credential://do-not-echo` is absent from the message. |
| AC-002.7 | SPEC-002 | `tests/project_schema.rs::rejects_files_larger_than_64_kib_before_parsing`; M-12 | Pass | One violation naming the file and the 64 KiB limit; size is checked before parsing (EDGE-013). |
| AC-003.1 | SPEC-003 | `tests/project_notice.rs::untrusted_files_are_inert_except_for_one_notice`; `tests/project_facade.rs::lifecycle_approves_invalidates_and_revokes_a_project_file`; M-07 | Pass | A never-approved pin has no effect on selection. |
| AC-003.2 | SPEC-003 | `tests/project_status.rs::allow_and_revoke_render_their_outcomes`; `tests/project_facade.rs::lifecycle_approves_invalidates_and_revokes_a_project_file`; M-03, M-04 | Pass | `allow` names the path and what approval enables; subsequent commands honor the pin. |
| AC-003.3 | SPEC-003 | `tests/project_trust.rs::a_fingerprint_changes_with_any_byte_change`; `tests/project_facade.rs::lifecycle_approves_invalidates_and_revokes_a_project_file`; M-07 | Pass | A trailing newline alone returns the file to `untrusted-changed`. |
| AC-003.4 | SPEC-003 | `tests/project_status.rs::allow_and_revoke_render_their_outcomes`; M-09 | Pass | Second `revoke` exits 0 and reports that no approval existed. |
| AC-003.5 | SPEC-003 | `tests/project_facade.rs::invalid_content_outranks_a_stale_approval_and_cannot_be_allowed`; M-13 | Pass | Exit 2, every violation listed with the remedy, no approval recorded. |
| AC-003.6 | SPEC-003 | `tests/project_facade.rs::mutations_require_a_discovered_project_file`; M-14 | Pass | Exit 5; message names `.agentenv.toml` and the working-directory-and-ancestors scope. |
| AC-003.7 | SPEC-003 | `tests/project_trust.rs::lookup_matches_a_path_reached_through_a_symlinked_ancestor`; `tests/project_facade.rs::approval_uses_the_canonical_path_through_a_symlinked_ancestor` | Pass | Trust identity is the canonical path. |
| AC-003.8 | SPEC-003 | `tests/project_trust.rs::a_corrupt_store_names_the_path_and_a_next_action`; `tests/project_facade.rs::corrupt_trust_store_is_propagated_as_a_configuration_error`; `tests/project_status.rs::status_infrastructure_failures_leave_json_stdout_empty` | Pass | Exit 2 naming the store path; never treated as empty. |
| AC-003.9 | SPEC-003 | `tests/project_trust.rs::saving_creates_the_store_with_owner_only_permissions` | Pass | Unix-gated `0600` assertion at creation. |
| AC-003.10 | SPEC-003 | `tests/project_trust.rs::revoking_one_approval_preserves_the_other` | Pass | Snapshot-preserving mutation; the store still parses. |
| AC-003.11 | SPEC-003 | `src/project/trust.rs::tests::a_failing_commit_leaves_the_previous_store_intact` | Pass | Fault injected through the `StoreFs` seam; previous bytes intact, error names the store path. |
| AC-003.12 | SPEC-003 | `tests/project_trust.rs::approval_binds_the_snapshot_it_was_given` | Pass | Approval binds the validated bytes; later on-disk content resolves as untrusted. |
| AC-003.13 | SPEC-003 | `tests/project_trust.rs::revoking_one_approval_preserves_the_other`; M-16 | Pass | `revoke` is path-only, so an unparseable file is still revocable. |
| AC-004.1 | SPEC-004 | `tests/project_precedence.rs::trusted_pin_precedes_default_and_yields_to_env_and_flag`; M-04 | Pass | Pin selects the profile with no flag or environment selection. |
| AC-004.2 | SPEC-004 | `tests/project_precedence.rs::trusted_pin_precedes_default_and_yields_to_env_and_flag`; M-04 | Pass | `AGENTENV_PROFILE` outranks the pin. |
| AC-004.3 | SPEC-004 | `tests/project_precedence.rs::trusted_pin_precedes_default_and_yields_to_env_and_flag`; M-04 | Pass | `--profile` outranks the pin. |
| AC-004.4 | SPEC-004 | `tests/project_precedence.rs::dangling_trusted_pin_names_its_project_file`; M-15 | Pass | Exit 3; message names the project file and lists the defined profiles. |
| AC-004.5 | SPEC-004 | `tests/project_precedence.rs::trusted_pin_precedes_default_and_yields_to_env_and_flag`; M-04 | Pass | Pin outranks `default_profile` in both fixtures (opposite pin/default pairings). |
| AC-004.6 | SPEC-004 | `tests/project_notice.rs::invalid_untrusted_files_remain_inert_and_bypass_suppresses_discovery`; M-07 | Pass | An untrusted pin does not participate; `default_profile` wins. |
| AC-004.7 | SPEC-004 | `tests/project_precedence.rs::run_uses_the_trusted_pin_for_injection_planning` | Pass | `test-probe` observes the pinned profile's injected value. |
| AC-004.8 | SPEC-004 | `tests/project_precedence.rs::set_and_unset_follow_the_trusted_pin_but_create_profile_does_not` | Pass | `set`/`unset` write under the pinned profile; `--create-profile` keeps its explicit-flag usage error (EDGE-008). |
| AC-005.1 | SPEC-005 | `tests/project_notice.rs::untrusted_files_are_inert_except_for_one_notice`; M-01 | Pass | `list --json` stdout byte-identical to the bypassed run; exactly one stderr line; stdout parses as JSON (EDGE-010). |
| AC-005.2 | SPEC-005 | `tests/project_notice.rs::invalid_untrusted_files_remain_inert_and_bypass_suppresses_discovery` | Pass | Unparseable file: command succeeds as if absent, one notice. |
| AC-005.3 | SPEC-005 | M-17 | Pass | Approved file made unreadable: exit 2, message names the file and offers restore or `agentenv project revoke`. No automated test; verified manually. |
| AC-005.4 | SPEC-005 | `tests/project_notice.rs::invalid_untrusted_files_remain_inert_and_bypass_suppresses_discovery`; M-10 | Pass | Both bypass forms leave stderr empty. |
| AC-005.5 | SPEC-005 | `tests/project_notice.rs::notice_precedes_command_errors_and_is_absent_for_parse_failures` | Pass | Unrelated `get` failure keeps exit 3; notice precedes the error, exactly once. |
| AC-005.6 | SPEC-005 | `tests/project_notice.rs::notice_is_flushed_before_a_run_target_replaces_the_process` | Pass | Notice occupies the first stderr line, ahead of the target's own stderr. |
| AC-005.7 | SPEC-005 | `tests/project_notice.rs::trusted_files_and_unavailable_state_follow_their_notice_rules` | Pass | Command succeeds; notice names `XDG_STATE_HOME` and `HOME` (EDGE-004a). |
| AC-005.8 | SPEC-005 | `tests/project_notice.rs::trusted_files_and_unavailable_state_follow_their_notice_rules`; M-04 | Pass | Trusted file: stderr empty. |
| AC-005.9 | SPEC-005 | `tests/project_notice.rs::notice_precedes_command_errors_and_is_absent_for_parse_failures` | Pass | Unknown flag: exit 1, no notice, no discovery. |
| AC-006.1 | SPEC-006 | `tests/project_status.rs::status_json_matches_the_frozen_member_state_table` (`no-file` row, `tests/snapshots/project-status-no-file.json`); M-19 | Pass | Exit 0. |
| AC-006.2 | SPEC-006 | `tests/project_status.rs::status_json_matches_the_frozen_member_state_table` (`untrusted` row); M-02 | Pass | Exit 5; text report names the approval command. |
| AC-006.3 | SPEC-006 | `tests/project_status.rs::status_json_matches_the_frozen_member_state_table` (`invalid` row); M-12, M-16 | Pass | Exit 5; violations carry TOML/file paths and the remedy, no values. |
| AC-006.4 | SPEC-006 | `tests/project_status.rs::status_json_matches_the_frozen_member_state_table` (`checked` row); M-05, M-06 | Pass | Exit 0; every requirement reported satisfied with its reason. |
| AC-006.5 | SPEC-006 | `tests/project_status.rs::status_reports_unsatisfied_requirements_without_running_providers`; M-08 | Pass | Exit 6; missing entry and missing field named. |
| AC-006.6 | SPEC-006 | `tests/project_status.rs::status_json_matches_the_frozen_member_state_table` + all six `tests/snapshots/project-status-*.json`; M-05, M-20 | Pass | One JSON document per row, exact member set, stderr empty. |
| AC-006.7 | SPEC-006 | M-18 | Pass | No user config: `version` is `null`, requirements not checked with the reason and next action, exit 6 with requirements and exit 0 without. Automated coverage exercises the unparseable-config variant; the missing-config variant is manual. |
| AC-006.8 | SPEC-006 | M-15 | Pass | Dangling pin: report states the pin and that the pinned profile is not defined; exit 6 with requirements declared. No automated test; verified manually. |
| AC-006.9 | SPEC-006 | `tests/project_status.rs::status_json_matches_the_frozen_member_state_table` (`degraded` row); M-21 | Pass | Unparseable user config: report produced, `version` null, exit 0 with zero requirements and 6 with requirements declared. |
| AC-006.10 | SPEC-006 | M-22 | Pass | No pin, no `default_profile`, requirements declared: reason states no profile was selectable, exit 6. No automated test; verified manually. |
| AC-006.11 | SPEC-006 | `tests/project_status.rs::status_json_matches_the_frozen_member_state_table` (`unavailable` row, via `run_without_state_base`) | Pass | `trust: unavailable`, `trust_reason` names the location and variables, no notice, exit 5. |
| AC-006.12 | SPEC-006 | `tests/project_status.rs::status_infrastructure_failures_leave_json_stdout_empty` | Pass | Exit 2, stdout empty, stderr names `trust.toml`. |
| AC-006.13 | SPEC-006 | `tests/project_facade.rs::invalid_content_outranks_a_stale_approval_and_cannot_be_allowed`; M-16 | Pass | Edited-into-unparseable classifies `invalid` with violations, exit 5 (EDGE-011). |
| AC-007.1 | SPEC-007 | `tests/project_status.rs::status_json_matches_the_frozen_member_state_table` (`checked` row); M-05 | Pass | Entry present and every declared field resolvable. |
| AC-007.2 | SPEC-007 | `tests/project_status.rs::status_reports_unsatisfied_requirements_without_running_providers`; M-08 | Pass | Missing entry reported as `entry <name>`. |
| AC-007.3 | SPEC-007 | `tests/project_status.rs::status_reports_unsatisfied_requirements_without_running_providers`; M-23 | Pass | Unresolvable field path named in `missing`. |
| AC-007.4 | SPEC-007 | `tests/project_status.rs::status_reports_unsatisfied_requirements_without_running_providers`; `tests/project_security.rs::ac_010_4_project_operations_do_not_execute_counting_providers` | Pass | The counting provider's counter file is never created. |
| AC-007.5 | SPEC-007 | `tests/project_security.rs::ac_010_5_trusted_pin_selects_exactly_the_pinned_profiles_injection_plan`; M-08 | Pass | `get` returns normally and exits 0 while a requirement is unsatisfied. |
| AC-007.6 | SPEC-007 | M-23 | Pass | `fields = ["auth.endpoint"]` satisfied against a nested table; `auth.missing` unsatisfied and named. No automated test for the dotted path; verified manually. |
| AC-007.7 | SPEC-007 | `tests/project_status.rs::status_json_matches_the_frozen_member_state_table` (`checked` row: `fields = ["auth", "credential"]` and `["settings"]`); M-20 | Pass | Tables and credential references both satisfy. |
| AC-008.1 | SPEC-008 | `reports/task-T010-report.md` walkthrough; M-01..M-11 | Deferred | Every documented `project` command behaves as written. The Docker Compose pairing example could not be executed locally. See Deferred Items DEF-001. |
| AC-008.2 | SPEC-008 | `README.md` lines 497-511 compared with observed statuses; M-01..M-23 | Pass | Statuses 0, 1, 2, 3, 5, 6 observed as documented in this run; 4 and 127 keep their pre-existing meaning and are unchanged by this change. |
| AC-008.3 | SPEC-008 | `skills/agentenv/SKILL.md` line 46 | Pass | Reading protocol step 1 is `agentenv project status --json`, ahead of profile-dependent reads. |
| AC-009.1 | SPEC-009 | `cargo test`: 223 passed, 0 failed | Pass | Pre-existing assertions unmodified; only the mechanical hermeticity changes from T001. |
| AC-009.2 | SPEC-009 | `git diff 3cd129b..85cc727 -- tests/snapshots/` shows six added files and zero modified lines; `tests/query_p1.rs::json_shapes_are_frozen_by_snapshots` | Pass | Every pre-existing snapshot is byte-identical. |
| AC-010.1 | SPEC-010 | `tests/project_security.rs::ac_010_1_invalid_project_values_never_reach_allow_status_or_notices` | Pass | Five sentinels in forbidden positions; absent from every stdout and stderr. |
| AC-010.2 | SPEC-010 | `tests/project_security.rs::ac_010_2_untrusted_profile_and_reason_values_never_reach_regular_commands` | Pass | Untrusted `profile` and `reason` sentinels never leave the file. |
| AC-010.3 | SPEC-010 | M-20 | Pass | User-config field values, nested values, entry and profile descriptions, and credential definition members all carry sentinels; the full report renders every envelope member and leaks none. No automated test; verified manually. |
| AC-010.4 | SPEC-010 | `tests/project_security.rs::ac_010_4_project_operations_do_not_execute_counting_providers` | Pass | Execution count unchanged across `status`, `allow`, and `revoke`. |
| AC-010.5 | SPEC-010 | `tests/project_security.rs::ac_010_5_trusted_pin_selects_exactly_the_pinned_profiles_injection_plan` | Pass | Injected names and sources are exactly the pinned profile's; the project file contributes none. |

Totals: 72 acceptance criteria — 71 Accepted, 1 Deferred (AC-008.1).

## Local Verification Commands

Plan gates, run directly and read in full:

| Gate | Result | Output summary |
| --- | --- | --- |
| `cargo build` | Pass | Exit 0. |
| `cargo test` | Pass | 223 passed, 0 failed, 0 ignored, across 17 test binaries plus doc-tests. |
| `cargo fmt --check` | Pass | Exit 0, no output. |
| `git status --porcelain -- src/` (T001 gate) | Pass | Empty. |
| `git status --porcelain -- src/ tests/` (T010 gate) | Pass | Empty. |
| `git diff 3cd129b..85cc727 -- tests/snapshots/` | Pass | Six files added, zero lines modified. |

Below is the machine record from `sdd.py verify --compare-baseline --update-validation`, reproduced as generated. Two reading notes: its `cargo test` summary is the last line of the run (the doc-test binary), not the whole-suite total of 223; and nine of its nineteen entries are not commands, as the triage below explains.

| Command | Result | Output summary |
| --- | --- | --- |
| `cargo test` | pass | test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.11s |
| `src/` | fail (exit 126) | /bin/sh: src/: is a directory |
| `git status --porcelain -- src/` | pass |  |
| `cargo test --test project_schema` | pass | test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s |
| `cargo test project::locate` | pass | test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 5 filtered out; finished in 0.00s |
| `cargo test --test project_trust` | pass | test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s |
| `todo!()` | fail (exit 2) | /bin/sh: -c: line 1: syntax error: unexpected end of file |
| `cargo build` | pass | Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.03s |
| `cargo fmt --check` | pass |  |
| `cargo test --test project_facade` | pass | test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s |
| `XDG_STATE_HOME` | fail (exit 127) | /bin/sh: XDG_STATE_HOME: command not found |
| `cargo test --test project_precedence --test project_notice` | pass | test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s |
| `test-probe` | fail (exit 127) | /bin/sh: test-probe: command not found |
| `set` | pass | __CF_USER_TEXT_ENCODING=0x1F5:0x0:0x0 |
| `cargo test --test project_status` | pass | test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.48s |
| `git status --porcelain -- src/ tests/` | pass |  |
| `project` | fail (exit 127) | /bin/sh: project: command not found |
| `python <package-root>/scripts/sdd.py verify 003-project-config --compare-baseline --update-validation` | fail (exit 1) | /bin/sh: package-root: No such file or directory |
| `validation.md` | fail (exit 127) | /bin/sh: validation.md: command not found |

## Failure Triage

| Command | Classification |
| --- | --- |
| `cargo test` | pass |
| `src/` | pre-existing failure |
| `git status --porcelain -- src/` | pass |
| `cargo test --test project_schema` | fixed pre-existing failure |
| `cargo test project::locate` | pass |
| `cargo test --test project_trust` | fixed pre-existing failure |
| `todo!()` | pre-existing failure |
| `cargo build` | pass |
| `cargo fmt --check` | pass |
| `cargo test --test project_facade` | fixed pre-existing failure |
| `XDG_STATE_HOME` | pre-existing failure |
| `cargo test --test project_precedence --test project_notice` | fixed pre-existing failure |
| `test-probe` | pre-existing failure |
| `set` | pass |
| `cargo test --test project_status` | fixed pre-existing failure |
| `git status --porcelain -- src/ tests/` | pass |
| `project` | pre-existing failure |
| `python <package-root>/scripts/sdd.py verify 003-project-config --compare-baseline --update-validation` | pre-existing failure |
| `validation.md` | pre-existing failure |

Zero new failures against the pre-implementation baseline.

Reclassification of the generated rows above:

| Entry | Actual classification |
| --- | --- |
| `cargo test`, `cargo build`, `cargo fmt --check`, both `git status` checks, `cargo test project::locate` | pass |
| `cargo test --test project_schema`, `--test project_trust`, `--test project_facade`, `--test project_precedence --test project_notice`, `--test project_status` | new passing tests |
| `src/`, `todo!()`, `XDG_STATE_HOME`, `test-probe`, `project`, `set`, `validation.md`, `python <package-root>/scripts/sdd.py verify …` | not a command |

The five rows the generator labels `fixed pre-existing failure` are the project test binaries. They failed in the baseline because the test files did not yet exist; they now exist and pass, so they are new tests rather than repaired regressions.

The nine `not a command` entries are artifacts of how the log is assembled: backtick-quoted fragments of task prose — file paths, a Rust macro, an environment variable name, a binary name, a subcommand word, a documentation filename, and a command template still holding its `<package-root>` placeholder — were collected alongside the real `Verification:` commands and executed by a shell. None is a gate, and none reflects product behavior. Two consequences are worth recording. First, the phantom failures inflate the failure count and would mask a genuine regression in the same run. Second, the fragment `set` executed as the shell builtin and dumped the whole process environment, including live credential values, into `reports/verification-log.md`; the same dump is present in `reports/baseline-log.md` and in the archived `002-002-config-write` logs. Those files match `.sdd/.gitignore` (`changes/**/reports/*log*.md`) and are never committed, so the exposure is confined to the local working tree — but the logs should be deleted rather than kept, and the extraction step should take only whole `Verification:` commands. Recorded as DEV-001.

## Manual Validation

Scenarios ran against `target/debug/agentenv` in scratch trees created with `mktemp -d`, with `XDG_STATE_HOME` and `AGENTENV_FILE` pointed inside the tree and `AGENTENV_NO_PROJECT` unset. Commands ran from a nested subdirectory of the project root.

| Scenario | Steps | Result | Notes |
| --- | --- | --- | --- |
| M-01 Untrusted inertness | `list --json` in a tree with a new `.agentenv.toml`, compared against `--no-project list --json` | Pass | stdout byte-identical; exactly one stderr line naming the file and `agentenv project status`; stdout parses as JSON. |
| M-02 Untrusted status | `project status` | Pass | Exit 5; reports `untrusted-new`, the pin, and the approval command. |
| M-03 Approval | `project allow` | Pass | Exit 0; names the file and what approval enables. |
| M-04 Pinned selection | `get llm.model`, `list`, then `--profile work` and `AGENTENV_PROFILE=work` | Pass | Pin `personal` beats `default_profile = "work"`; flag and environment each override it; stderr empty throughout. |
| M-05 Frozen envelope | `project status --json` while trusted and satisfied | Pass | Exit 0; exactly the members `version`, `project.{discovered,path,trust,trust_reason,violations,profile_pin,requirements}`, `requirements.{checked,reason,profile,entries}`, `entries[].{entry,reason,satisfied,missing}`; stderr empty. |
| M-06 Text report | `project status` while trusted and satisfied | Pass | Exit 0; same members in prose form. |
| M-07 Edit invalidates | Append a newline to the approved file, then `project status --json` and `get llm.model` | Pass | `untrusted-changed`, status exit 5; the pin stops applying and `default_profile` wins again, with the notice restored. |
| M-08 Unsatisfied requirement | Declare `[requires.kubernetes]` against a profile lacking it, `allow`, then `project status` and `get` | Pass | Status exit 6 naming `entry kubernetes`; `get` unaffected, exit 0. |
| M-09 Revocation | `project revoke` twice, then `project status` and `get` | Pass | First revoke reports removal; second reports no record, exit 0; file inert again with the notice. |
| M-10 Bypass | `AGENTENV_NO_PROJECT=1 get`, then `AGENTENV_NO_PROJECT= get` | Pass | Bypassed run emits no stderr; the empty value counts as unset and the notice returns. |
| M-11 Bypass excludes the project group | `AGENTENV_NO_PROJECT=1 project status` | Pass | Exit 5 — discovery still ran. |
| M-12 Oversized file | 70 KB `.agentenv.toml`, `project status` | Pass | `invalid` with one violation naming the file and the 64 KiB limit; exit 5. |
| M-13 Allow on an invalid file | `project allow` on a file with a bad `version` and an `inject` table | Pass | Exit 2; both violation paths listed; the file stays untrusted. |
| M-14 Mutations with no file | `project allow` and `project revoke` in a tree with no project file | Pass | Both exit 5, naming `.agentenv.toml` and the working-directory-and-ancestors scope. |
| M-15 Dangling pin | Trusted pin naming an undefined profile: `project status`, then `get llm.model` | Pass | Status exit 6 with the reason and next action; `get` exit 3 naming the project file and listing the defined profiles. |
| M-16 Edited into unparseable | Approve, overwrite with invalid TOML, then `project status` and `project revoke` | Pass | Classified `invalid` (not `changed`) with a violation and no source echo, exit 5; revoke still succeeds. |
| M-17 Unreadable approved file | Approve, `chmod 000`, then `get llm.model` | Pass | Exit 2; names the file and offers restore or `agentenv project revoke`. |
| M-18 No user config | Trusted file with a requirement and no config file: `project status`, then the same without requirements | Pass | `version` null, requirements not checked with reason and next action, exit 6; exit 0 when no requirement is declared. |
| M-19 File deleted after approval | Approve, delete the file, then `get` and `project status` | Pass | `get` exits 0 with empty stderr; status reports no file discovered, exit 0. The stale record is harmless. |
| M-20 No-secret invariant under a full report | Config carrying sentinels in field values, a nested value, profile and entry descriptions, and every credential definition member; trusted file with three declared `fields` | Pass | `allow` and both `status` forms render the complete envelope and leak no sentinel. Also confirms a nested field path and a credential reference satisfy. |
| M-21 Unparseable user config | Trusted file with a requirement and invalid config TOML: `project status --json` | Pass | Report produced, `version` null, requirements not checked, exit 6. |
| M-22 No selectable profile | No pin, config without `default_profile`, one requirement declared | Pass | Reason states no profile was selectable and names the next action; exit 6. |
| M-23 Nested field paths | `fields = ["auth.endpoint"]` against a nested table, then `fields = ["auth.missing"]` | Pass | Satisfied, exit 0; unsatisfied naming `auth.missing`, exit 6. |

### Startup latency (SPEC-AS-007)

Measured on macOS 15.6 (Darwin 25.6.0, Apple Silicon) with `target/debug/agentenv`. A trusted `.agentenv.toml` sat at the root of a tree 20 directories deep; `agentenv list` ran from the deepest directory. Each cell is 200 timed invocations after 20 warm-up runs, and the two conditions were interleaved over three rounds to cancel machine drift. `hyperfine` is not installed on this machine, so timing used `time.perf_counter` around `subprocess.run`, which includes process spawn.

| Round | Condition | min (ms) | median (ms) | mean (ms) | p95 (ms) |
| --- | --- | ---: | ---: | ---: | ---: |
| 1 | discovery | 2.877 | 3.050 | 3.048 | 3.157 |
| 1 | bypass (`AGENTENV_NO_PROJECT=1`) | 2.764 | 2.922 | 2.928 | 3.045 |
| 2 | discovery | 2.884 | 3.056 | 3.063 | 3.206 |
| 2 | bypass | 2.748 | 2.938 | 2.949 | 3.156 |
| 3 | discovery | 2.886 | 3.049 | 3.066 | 3.190 |
| 3 | bypass | 2.778 | 2.931 | 2.946 | 3.128 |

Aggregate median 3.051 ms with discovery against 2.930 ms bypassed; aggregate minimum 2.877 ms against 2.748 ms. Discovery overhead is 0.12 ms, consistent to within 0.01 ms across all three rounds.

Conclusion: discovery overhead is not perceptible — 0.12 ms across a 20-deep walk plus the file read, hash, and trust-store lookup, roughly 4 percent of a run that is dominated by process startup and about eighty times under the 10 ms expectation.

## Known Deviations

| ID | Deviation | Impact | Decision |
| --- | --- | --- | --- |
| DEV-001 | The verification log's command extraction collects backtick-quoted prose fragments from `tasks.md` and runs them as shell commands, producing seven phantom failures and — through the fragment `set` — an environment dump containing live credential values in `reports/verification-log.md` and `reports/baseline-log.md`. | No product impact; `src/` and `tests/` are unaffected. The phantom failures would mask a real regression in the same run. The dumped values are confined to gitignored local files (`.sdd/.gitignore`, `changes/**/reports/*log*.md`) and are never committed. The same dump exists in the archived `002-002-config-write` logs. | Accept for this change; the report logs should be deleted from the working tree, and the extraction step should be narrowed to whole `Verification:` commands. Tooling fix, outside this change's scope. |
| DEV-002 | Eight acceptance criteria have no dedicated automated test and rest on manual evidence: AC-001.5, AC-005.3, AC-006.7 (missing-config half), AC-006.8, AC-006.10, AC-007.6, AC-010.3, and the exit-status comparison in AC-008.2. | All eight pass. AC-005.3 and AC-010.3 sit on security-relevant surfaces — the exit-2 unreadable-approved-file path and the no-secret invariant over a full report — so they will not be caught by CI if they regress. | Accept for this change; add regression tests for AC-005.3 and AC-010.3 in a follow-up. |
| DEV-003 | The degraded-selection reason in `project status` nests two clauses: "Requirements: not checked — requirements could not be checked because name resolution error: …; fix the selection and run `agentenv project status`". | Wording only. The line names the reason and the next action as SPEC-006 requires. | Accept; the wording is not contractual (SPEC-AS-003 applies the same principle to the notice). |

Resolution note (2026-08-28, post-archive): the DEV-002 follow-up for the two security-relevant criteria is complete. `tests/project_security.rs::ac_005_3_unreadable_approved_project_file_fails_loudly_with_exit_2` covers the unreadable-approved-file path (exit 2, file named, next action present, empty stdout), and `tests/project_security.rs::ac_010_3_status_renders_the_full_envelope_without_config_values` plants sentinels in open-schema field values, nested values, profile and entry descriptions, and credential definition members, then asserts both the JSON and text `project status` reports render the envelope (version, active profile, requirement entry, reason) with no sentinel on either channel. The remaining six manual-evidence criteria in DEV-002 stay accepted as-is.

## Deferred Items

| Item | Reason | Follow-up |
| --- | --- | --- |
| DEF-001: the Docker Compose walkthrough in AC-008.1 | Docker Compose is not installed on this machine; the local Docker CLI reports `unknown command: docker compose`, so the documented `agentenv run --with llm -- docker compose up` example could not be executed end to end. `agentenv` reached and invoked the target as documented (`reports/task-T010-report.md`). The mechanism the example relies on — credentials reaching a child process through the environment, and nothing else — is verified by `tests/project_security.rs::ac_010_5_trusted_pin_selects_exactly_the_pinned_profiles_injection_plan`, which asserts the injected names and sources through the probe target. Remaining risk: none beyond standard Compose interpolation semantics. | Run the walkthrough once on a machine with Docker Compose installed. |

Resolution (2026-08-28, post-archive): DEF-001 is resolved. The walkthrough ran end to end on this machine with Docker Compose 5.5.0 against daemon 29.5.2, in a scratch tree matching the README's "Docker Compose pairing" section: a `config.toml` whose `llm` entry carries a `command`-provider credential (`inject_as = "OPENAI_API_KEY"`) and an `inject` value (`OPENAI_BASE_URL`), a `.env` holding only the non-secret `IMAGE_TAG=3.20`, and a `compose.yaml` using both documented mechanisms. `agentenv run --with llm -- docker compose config` rendered `image: alpine:3.20` from `.env`, `OPENAI_API_KEY: walkthrough-fake-token` via environment passthrough, and the entry's endpoint via `${OPENAI_BASE_URL}` interpolation. `agentenv run --with llm -- docker compose up` then started the container, which observed the passthrough variable set (length 22) and the interpolated value intact; exit status 0. The negative control without `agentenv` produced Compose's "variable is not set" warning and a null `OPENAI_API_KEY`, confirming credentials reach Compose only through `agentenv run`. AC-008.1 therefore stands fully accepted; the walkthrough used a fake token only and no secret value appeared in any output.

## Final Decision

Decision: Accepted

Rationale:

- All 72 acceptance criteria are resolved: 71 accepted with evidence, 1 (AC-008.1) deferred with the rationale and residual risk recorded in DEF-001. No criterion is unaddressed.
- `cargo build`, `cargo fmt --check`, and `cargo test` all pass; 223 tests, zero failures. No new failures against the pre-implementation baseline, and every pre-existing snapshot is byte-identical.
- The end-to-end lifecycle was walked in scratch trees: an untrusted file is inert behind a single stderr notice, `allow` activates the pin, an edit invalidates it, `revoke` deactivates it, the JSON envelope matches the frozen contract, and unsatisfied requirements exit 6.
- Discovery costs 0.12 ms in a 20-deep tree, discharging SPEC-AS-007.
- DEV-001 is a verification-tooling defect with no product impact. It does not block archive, but the report logs should be deleted from the working tree first, since they hold credential values in plaintext.
