# Checkpoint review (Group 1) — round 1

Lane: Claude native high-capability code-review (clean context). Verdict: revise.

Findings and resolutions (all applied):

- Important: `set` silently replaced a structural table leaf with a scalar. → Table/array-of-tables leaves now refuse with exit 3 pointing at `agentenv unset`; regression test added.
- Important: inline tables were unaddressable (false exit-3 diagnostic) although the read path resolves them. → Traversal moved to `toml_edit::TableLike` (standard + inline); regression test added.
- Important: `--type json` integers above i64::MAX silently became lossy floats. → `is_u64` literals refused with the range usage error; regression test added.
- Important: diagnostics referenced `agentenv init` / `agentenv credential add` before those commands existed, and ~115 lines (`init`, `credential_add`) were unreachable/untested. → Resolved by landing T004/T005 (commands wired + 12 integration tests) in the same checkpoint, per the review's first recommended option.
- Minor (all applied): guardrail re-implemented as a post-validation message upgrade keyed on the validator's own violation phrase (single scope source; conflict precedence now correct; JSON-nested sensitive fields get the remedy); `toml_edit` parse-divergence diagnostic keeps `error.message()`; temp-file name gains a nanosecond component with an AlreadyExists hint; `file_mode` failures now surface instead of silently narrowing permissions, directory-fsync best-effort justified in a comment; `--description` with any single-segment path is a named usage error; test suite fixes (float/bool sentinel coverage, sentinel fixture in the unset suite, root guard + de-brittled assertion in the unwritable-dir test, shared `staged_config`/`read_config` helpers).
- `init`'s and `resolve_existing_target`'s canonicalize error handling now distinguishes dangling symlinks from other I/O errors.

Post-fix verification: 175 tests pass, `cargo fmt --check` and `cargo clippy --all-targets` clean. Consolidated re-review dispatched over the full working-tree diff.

## Round 2 (consolidated re-review, full working tree)

Verdict: revise. Round-1 items verified resolved except two follow-ups: the table-leaf protection missed the inline representation (`limits = { ... }` still silently replaced), and the JSON integer guard covered only the u64 band (literals above u64::MAX / below i64::MIN still became lossy floats). Four minors: stale `is_sensitive_name` visibility/doc, misleading traversal-conflict wording, AC-005.1 verification surface, `init` directory misreport.

All applied: inline-table leaves refused (exit 3); serde_json `arbitrary_precision` + literal-based classification closes every integer band; visibility reverted; `value_not_table` conflict message; AC-005.1 amended (file as surface, test asserts written provider fields); `init` directory branch added.

## Round 3 (final targeted re-check)

Verdict: approve-with-minor. All six round-2 items probe-verified, no JSON output-shape regression from arbitrary_precision. Three minors, all applied: `--type float` finite literals overflowing to inf now refused (explicit inf/nan keywords still accepted); `unset` traversal conflict reuses the value-not-table wording; Cargo.toml documents the arbitrary_precision constraint. Checkpoint accepted.
