# Architecture Design Document: Project-Scoped Configuration

## Source Artifacts

- Change ID: 003-project-config
- PRD: `.sdd/changes/003-project-config/prd.md`
- Related current specs: `.sdd/specs/current/001-agent-context-cli/spec.md`, `.sdd/specs/current/002-002-config-write/spec.md`
- Relevant code areas: `src/config/locate.rs`, `src/config/model.rs` (`select_profile`), `src/cli/mod.rs`, `src/config/write.rs`, `src/query/`, `skills/agentenv/SKILL.md`, `README.md`

## Current State

- One user-owned config file, resolved by `config::locate::resolve_path` (explicit path → `AGENTENV_FILE` → platform default). On Unix its permission bits must be a subset of `0600`.
- Profile selection is `Config::select_profile(flag, env_val)` (`src/config/model.rs:152`): `--profile` → `AGENTENV_PROFILE` → `default_profile`, with explicit errors listing options. Callers: the CLI dispatch (`src/cli/mod.rs:367`) and the write path (`src/config/write.rs:162`, `:444`).
- No notion of a project-level file, of working-directory context, or of trust state. The CLI is stateless apart from the config file and the platform credential store.
- The no-secret invariant: no command prints a resolved credential; diagnostics never echo TOML source lines or open-schema values.

## Goals

- Introduce a checked-in project file that can pin a profile and declare required entries/fields, discovered from the working directory.
- Gate every effect of that file behind user-owned trust-on-first-use approval of its exact content.
- Keep all existing behavior byte-identical when no project file exists, and keep the no-secret invariant untouched.

## Non-Goals

- Any project-file content beyond selection and declaration (no values, credentials, `inject` tables, or references) — PRD D-01.
- Emitting `.env` files or exporting values into a shell. Pairing with `.env`/compose is documentation of the existing `run` workflow.
- Multi-file layering or merging of several project files along the ancestor walk: the nearest file wins and is the only one considered.

## Proposed Architecture

A new `src/project/` module owns everything project-scoped: discovery (walk up from the CWD), parsing and closed-schema validation of the project file, and the user-owned trust store. It exposes one narrow product to the rest of the CLI: a `ProjectContext` value stating "no file", "file present but untrusted", or "trusted file with pin and requirements". `Config::select_profile` gains the trusted pin as one more fallback rung. A new `agentenv project` subcommand group (`status`, `allow`, `revoke`) manages and reports trust; a global `--no-project` flag and `AGENTENV_NO_PROJECT` skip discovery entirely. Untrusted or invalid files never change behavior; they produce a single stderr notice pointing at `agentenv project status`.

## System Context

```text
agent/user -> agentenv CLI -> project::discover (CWD walk-up)   -> .agentenv.toml (repo, untrusted input)
                           -> project::trust  (user state file) -> allow/revoke records (user-owned)
                           -> config::load    (user config)     -> Config::select_profile(flag, env, project_pin)
                           -> query/run/write (unchanged behavior, project-aware selection)
```

## Module Boundaries and Interfaces

| Module | Interface | Responsibilities | Dependencies | Notes |
| --- | --- | --- | --- | --- |
| `project::locate` | `discover(cwd, env) -> Option<ProjectFilePath>` | Walk from `cwd` up to the filesystem root; return the nearest `.agentenv.toml`. Pure path/env logic plus `is_file` checks; honors `--no-project`/`AGENTENV_NO_PROJECT` upstream (callers skip discovery). | std fs | Deterministic: first file found wins; no merging. Symlinked ancestors are not canonicalized during the walk; the found path is canonicalized once for trust identity. |
| `project::model` | `ProjectFile::parse(text) -> Result<ProjectFile, Violations>`; fields `version`, `profile: Option<String>`, `requires: Vec<Requirement>` | Closed-schema TOML parsing: `version = 1`, optional `profile`, optional `[requires.<entry>]` tables with mandatory `reason` and optional `fields: [String]`. Any other key, table, or type is a violation naming the offending path. | `toml` | Closed schema is the enforcement of selection-only (PRD-FR-006). Diagnostics name paths, never echo values (house rule). |
| `project::trust` | `TrustStore::status(path, content) -> Trusted \| UntrustedNew \| UntrustedChanged`; `allow(path, content)`; `revoke(path)` | Persist approvals as `(canonical path, SHA-256 of content)` in a user-owned TOML state file; compare on read. Create parent dirs on first `allow`; `0600` file mode on Unix. | `sha2`, std fs | State file location: `$XDG_STATE_HOME/agentenv/trust.toml`, else `~/.local/state/agentenv/trust.toml`; Windows `%LOCALAPPDATA%\agentenv\trust.toml`. Never stored in the repo. |
| `project` (facade) | `ProjectContext::resolve(cwd, env) -> Result<ProjectContext, AppError>`; `ProjectContext` = `None \| Untrusted { path, reason } \| Trusted { path, pin: Option<ProjectPin>, requires }` | Compose locate → read → parse → trust check into the one value the CLI consumes. Fallible: a corrupt trust store and a trusted-but-unreadable file (approval record exists for the canonical path) are errors (exit 2); an unresolvable state base on read paths degrades to `Untrusted` with the reason; an unreadable/unparseable file without an approval record is `Untrusted`. Full content diagnostics surface via `project status` and `allow`. | locate, model, trust | The single seam the rest of the CLI sees. Adapters: production (real fs/env) and tests (temp dirs + injected env), matching the existing `locate.rs` test style. |
| `config::model::select_profile` | `select_profile(flag, env_val, project_pin: Option<&ProjectPin>)` where `ProjectPin { name: String, file: PathBuf }` | Insert the trusted pin between `AGENTENV_PROFILE` and `default_profile` (PRD-FR-002). A pin naming an unknown profile is the existing unknown-profile error (exit 3) whose message names the pin's originating project file. | existing | All three call sites (`cli/mod.rs`, `config/write.rs` ×2) pass the pin from `ProjectContext`; the source-tagged pin is what lets the error name the file. |
| `cli::project` | `agentenv project status [--json]`, `agentenv project allow`, `agentenv project revoke` | `status`: full report (discovery result, trust state, pin, per-requirement satisfied/unsatisfied), always produced even when the user config is missing/invalid or the pin dangles — degraded parts are reported as "not checked" with a reason. `allow`: validate content, then record trust; refuses an invalid file (exit 2). `revoke`: remove the record. `allow`/`revoke` with no discovered file: exit 5. | project, config | `status --json` is the agent surface; envelope frozen in the spec (SPEC-006). Command outcomes carry stdout, stderr, and exit status explicitly (the current `Result<Output, AppError>` shape cannot express a full report with a non-zero exit); the dispatch layer in `main.rs`/`cli/mod.rs` gains that outcome type. |

## Data Model and State

| Entity / State | Owner | Lifecycle | Validation rules | Persistence |
| --- | --- | --- | --- | --- |
| Project file (`.agentenv.toml`) | Repository (untrusted input) | Checked in; edited like source | Closed schema: `version = 1`; optional `profile` (non-empty string); `[requires.<entry>]` with mandatory non-empty `reason`, optional `fields` array of field paths | Repo file; never written by agentenv |
| Trust record | User (`trust.toml` in state dir) | Created by `allow`; invalidated by any content change (hash mismatch) or `revoke`; keyed by canonical absolute path | Path must be canonical; hash is SHA-256 over exact file bytes | User-owned TOML, `0600` on Unix |
| `ProjectContext` | `project` facade | Computed per invocation; never cached across processes | n/a | In-memory only |
| Requirement report | `cli::project` | Computed by `status` against the active profile | Entry must exist in the selected profile; each listed field path must resolve within the entry | Output only |

## External Interfaces

| Interface | Consumer | Contract | Compatibility / versioning |
| --- | --- | --- | --- |
| `.agentenv.toml` schema | Repos, agents, humans | `version = 1`; selection/declaration only; closed schema | New keys require a version bump; unknown keys are errors, not warnings |
| `agentenv project status --json` | Agents | Stable JSON: discovery, trust state, pin, requirements with satisfied flags and reasons | Additive evolution only, like existing JSON surfaces |
| Profile precedence | All commands | `--profile` > `AGENTENV_PROFILE` > trusted pin > `default_profile` | Strict superset of today's chain; unchanged without a project file |
| Untrusted-file notice | Humans (stderr) | One line naming the file and `agentenv project status`; emitted by every command except the `project` subcommands, `--help`, `--version`, and bypassed invocations — on success and failure paths alike, and before `run` starts or replaces the process | stderr only; never on stdout, never in `--json` payloads |
| Exit statuses | Agents, scripts | New status `5`: project trust-state failure (`status` on an untrusted/invalid file; `allow`/`revoke` with no discovered file). New status `6`: trusted file whose requirements are unsatisfied or uncheckable (`status` only). Status `2` keeps its meaning (configuration-file error) and also covers project-file validation errors. Existing statuses keep their meanings | Documented extension of the README table |

## Decisions and Alternatives

### Decision ARCH-001: Project file name and discovery

- Decision: `.agentenv.toml`, discovered by walking from the CWD to the filesystem root; the nearest file wins and is the only one considered.
- Rationale: matches direnv/mise discovery expectations; walk-to-root is deterministic, cheap (one `is_file` per ancestor), and needs no VCS knowledge.
- Alternatives considered:
  - CWD only: breaks the common case of agents running in subdirectories.
  - Git-root only: couples agentenv to git and fails in non-git trees.
  - Merging all ancestor files: layering semantics for marginal value; rejected for complexity and auditability.
- Consequences: a file in `$HOME` acts as a user-level pin for everything below it — acceptable because `$HOME` is user-owned and the trust gate still applies.

### Decision ARCH-002: Trust store — user state file with content hash

- Decision: approvals recorded as `(canonical path, SHA-256 of exact content)` in a user-owned `trust.toml` under the platform state directory; mismatch or absence means untrusted.
- Rationale: exactly the proven direnv/mise model; user-owned so a repo can never self-approve; content hash makes every edit visible.
- Alternatives considered:
  - Storing approvals in the user config file: pollutes a hand-edited file with machine state and makes `set`/`unset` interact with trust; rejected.
  - A git-ignored marker inside the repo: repo-owned state is attacker-adjacent and survives cloning tricks; rejected.
  - Trusting by path only (no hash): silent post-approval edits take effect; defeats PRD-FR-004; rejected.
- Consequences: new `sha2` dependency; moving a repo or re-cloning requires re-approval (accepted, matches direnv). Store mutations are atomic (0600-first temp file + rename): an interrupted mutation leaves the previous store intact, and concurrency is last-writer-wins per whole-store mutation with no cross-process locking (documented).

### Decision ARCH-003: CLI surface — `project` subcommand group

- Decision: `agentenv project status [--json]`, `agentenv project allow`, `agentenv project revoke`; global `--no-project` flag and `AGENTENV_NO_PROJECT` env bypass.
- Rationale: groups the new surface under one noun (mirrors `credential …`); `allow` matches direnv's verb, and `status` is the agent-facing report the skill will reference.
- Alternatives considered:
  - Top-level `agentenv allow`: pollutes the root namespace and reads ambiguously next to credential commands.
  - mise-style `trust` verb: `trust` collides conceptually with credential trust; rejected for vocabulary hygiene.
- Consequences: the skill and README gain one discovery step (`project status --json`) at the top of the protocol.

### Decision ARCH-004: Untrusted and invalid files are inert plus one stderr notice

- Decision: while a discovered file is untrusted (new, changed, or — absent an approval record — unreadable or unparseable), all commands outside the `project` subcommand group behave exactly as if it were absent, except for a single stderr notice naming the file and pointing at `agentenv project status`. The notice is emitted on success and failure paths alike, and for `run` before the target starts; `project` subcommands, `--help`, `--version`, and bypassed invocations never emit it. Full diagnosis (parse errors, forbidden keys, requirement details) appears only in `project status` and `allow` output. A file with an approval record for its canonical path that later fails to read is an error (exit 2), never a silent skip; an unresolvable state base on read paths degrades the file to untrusted with the reason in the notice, while `allow`/`revoke` fail explicitly (exit 2).
- Rationale: PRD-FR-004/FR-007 demand inertness with explicit reporting; pushing rich diagnostics into `status` keeps per-turn agent invocations quiet and keeps stdout/JSON stable. The trusted-but-unreadable case is a real failure of an approved input, so it must stop, per the no-silent-fallback rule.
- Alternatives considered:
  - Failing every command when an untrusted file is present: turns cloning any repo with a project file into breakage; hostile to the P1 story.
  - No notice at all: silent ignoring violates PRD-FR-007.
- Consequences: the notice is the one behavior change visible in trees with unapproved files; it must never appear on stdout or inside `--json` output.

### Decision ARCH-005: `select_profile` gains a third source rather than a pre-resolution wrapper

- Decision: extend `Config::select_profile(flag, env_val)` to `select_profile(flag, env_val, project_pin)` and update its three call sites, keeping the existing error catalogue.
- Rationale: the precedence chain lives in exactly one function today; adding the rung there keeps one owner for selection logic and reuses the unknown-profile error path (exit 3) with the project file named in the message.
- Alternatives considered:
  - A wrapper that rewrites `env_val` before calling: hides the pin's identity from error messages and confuses the empty-value rules; rejected.
  - Resolving the pin inside `ProjectContext` into a profile reference: moves profile knowledge into the project module and inverts the dependency; rejected.
- Consequences: a signature change ripples through three call sites and their tests; mechanical and contained.

### Decision ARCH-006: Requirement checking is structural and read-only

- Decision: `project status` reports, per declared requirement, whether the entry exists in the active profile and whether each declared field path resolves within it (field paths are entry-relative in the accepted segment grammar, resolved as `inject`-table paths are). It performs no credential resolution and no shallow provider checks, and no other command is blocked by unsatisfied requirements. `status` never fails on degraded selection (missing/invalid user config, dangling pin): it reports requirements as "not checked" with the reason and signals via exit status 6.
- Rationale: keeps `status` side-effect-free and fast (agent-callable every turn); credential availability already has a dedicated surface (`credential check`). Blocking unrelated reads on unsatisfied requirements would punish partial setups.
- Alternatives considered:
  - Including shallow credential status per requirement: duplicates `credential list` semantics and adds provider-dependent behavior to a structural report; rejected for v1.
  - A `--strict` mode failing `run` on unsatisfied requirements: deferred; recorded as a possible follow-up, not built speculatively.
- Consequences: agents combine `project status --json` with existing `credential` commands; documented in the skill update.

## Risks and Mitigations

| Risk | Impact | Mitigation | Owner |
| --- | --- | --- | --- |
| Trust bypass via path aliasing (symlinks, case-insensitive filesystems) | A changed file passes as trusted, or an approval fails to match | Canonicalize the discovered path once (`fs::canonicalize`) before hashing and lookup; hash exact bytes; integration tests cover symlinked project roots and edited files | Implementation |
| Notice leaking into machine-readable output | Agents parse corrupted JSON | Notice goes to stderr only; snapshot tests assert stdout byte-stability with an untrusted file present | Implementation |
| Behavior drift in trees without a project file | Silent regression of the v1 contract | Full existing test suite must pass unchanged; new discovery code short-circuits on `None` before touching selection | Implementation |
| State-dir differences across platforms | Trust silently lands in the wrong place or fails on Windows | Mirror the `locate.rs` pattern: pure env-based resolution with unit tests per platform, explicit error when no base dir env is set | Implementation |
| CWD-dependent behavior surprises scripts that `cd` | A script changes behavior after `cd` into a trusted tree | Documented precedence; `--no-project`/`AGENTENV_NO_PROJECT` escape hatch; notice makes presence visible | Docs |

## Testing Strategy

- Acceptance gates: integration tests via `assert_cmd` (existing style, `tests/` with fixtures), covering discovery, trust lifecycle, precedence, closed-schema rejection, requirement reporting, and byte-stability without a project file.
- Automated tests: unit tests inside `project::locate` / `project::trust` mirroring `config::locate`'s injected-env style; JSON snapshot for `project status --json` following `tests/snapshots/`.
- Manual validation: one end-to-end pass on macOS (author machine) plus CI matrix (Linux, Windows) as with changes 001/002.
- TDD seams: the trust gate (`project::trust` semantics: untrusted-new, untrusted-changed, allow, revoke, canonical-path identity) is a security-sensitive boundary — author its failing tests before implementation per the necessity trigger. Everything else follows the normal acceptance-gate style.

## Rollout, Migration, and Rollback

- Rollout: ship in one minor release (0.2.0); README, skill, and `AGENTS.md` block updated in the same change; no config migration (user config schema untouched).
- Migration: none — absence of a project file is the compatibility mode, and the trust store is created lazily on first `allow`.
- Rollback: remove/ignore `.agentenv.toml` files or set `AGENTENV_NO_PROJECT=1`; the trust store is inert data if the feature is reverted.

## Open Questions

| ID | Question | Impact if unresolved | Resolution |
| --- | --- | --- | --- |
| ARCH-Q-001 | Should the untrusted-file notice repeat on every command, or once per shell session? | Notice fatigue vs. statelessness | Resolved: every command — the CLI is stateless by design; suppression would require state for marginal benefit. |
| ARCH-Q-002 | Windows `%LOCALAPPDATA%` vs `%APPDATA%` for the trust store | Wrong roaming semantics | Resolved: `%LOCALAPPDATA%` — trust records are machine-local by nature (they name absolute paths on this machine); roaming them is wrong. |

## Architecture Review Checklist

- [x] Module seams are explicit.
- [x] Interfaces include invariants, error modes, and important constraints.
- [x] Modules are deep: `ProjectContext::resolve` hides discovery, parsing, and trust behind one value; no pass-through modules.
- [x] Every port/seam has at least two justified adapters (production fs/env + injected-env/temp-dir tests, matching the existing `locate.rs` pattern).
- [x] Each module's test seam matches its dependency category (local filesystem — substitutable via temp dirs and injected env).
- [x] Rejected alternatives are documented.
- [x] Risks have mitigations or accepted rationale.
- [x] Testing strategy matches risk and acceptance criteria.
- [x] No speculative architecture is included without a PRD or spec need (strict-mode `run` explicitly deferred).
