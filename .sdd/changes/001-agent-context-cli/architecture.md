# Architecture Design Document: agent-context CLI

## Source Artifacts

- Change ID: 001-agent-context-cli
- PRD: prd.md (backfilled summary; `design-source.md` is the binding behavior contract)
- Related current specs: none (greenfield repository)
- Relevant code areas: none (empty repository)

## Current State

Empty repository. No prior code, no established patterns. Toolchain available: Rust (cargo/rustc via Homebrew), Python 3.12, Node 24.

## Goals

- A single native binary with millisecond startup, suitable for per-turn agent invocation.
- A configuration model that is schema-open (arbitrary user fields) with a strictly validated core (profiles, descriptions, credentials).
- A hard internal boundary between "reads config" and "resolves secrets": query code paths must be incapable of returning secret values by construction.
- Cross-platform credential store access (macOS Keychain, Windows Credential Manager, Linux secret-service) behind one seam.

## Non-Goals

- Config file writing/editing (out of scope per design doc §11).
- Daemon/agent-server mode, caching layers, or config watching.

## Proposed Architecture

A Rust CLI (`clap` derive) organized as a library crate plus a thin `main`. The library exposes deep modules along the natural seams: config loading/validation, path addressing, querying, credential providers, and process injection. Rendering (text vs JSON) is separated from computing query results so both outputs share one code path for redaction decisions.

## System Context

```text
agent / user -> agent-context CLI (clap)
    -> config loader (TOML file) -> Config model
    -> query engine  -> renderer (text | JSON)      [never touches secrets]
    -> credential resolver -> provider seam -> env | keychain (keyring crate) | command (execv)
    -> runner (run --with) -> injection plan -> exec target process (Unix) / spawn+wait (Windows)
```

## Module Boundaries and Interfaces

| Module | Interface | Responsibilities | Dependencies | Notes |
| --- | --- | --- | --- | --- |
| `config` | `Config::load(source: ConfigSource) -> Result<Config, LoadError>` where `LoadError ∈ {NotFound, Parse, Validation(Vec<Violation>)}`; `Config { version, profiles, credentials }`; `Config::select_profile(cli_flag, env) -> Result<&Profile>` | Locate file (flag/env/XDG/platform default), parse TOML preserving key order, run strict core validation (descriptions, references, provider fields, reserved keys, sensitive-name rules, `inject` table shape), profile resolution order | `toml` (preserve_order), `serde` | Invariant: a returned `Config` is fully validated; downstream modules never re-check structure. Validation collects and reports **all** violations (SPEC-002/SPEC-011), each carrying its config path string; every command surfaces the full list. |
| `path` | `EntryPath::parse(&str) -> Result<EntryPath>`; `resolve(&Profile, &EntryPath) -> Result<&Value>` | Dot-separated path grammar with double-quoted segments for keys containing dots/spaces; navigation over the TOML value tree | none (pure) | Pure, in-process; property of grammar: `parse(render(p)) == p`. |
| `query` | `list(&Profile) -> Listing`; `show(&Profile, name) -> EntryView`; `get(&Profile, &EntryPath) -> ValueView`; `find(&Config, scope, needle) -> Matches` | Compute structured query results; classify values (scalar/array/table/credential-reference); attach shallow credential status | `config`, `path`, `credential::shallow_status` | By construction receives no secret values — `ValueView` has no variant that can hold one. |
| `render` | `render_text(view) -> String`; `render_json(view) -> serde_json::Value` | All user-facing formatting for query results, stable JSON shapes (envelopes carry config `version`; `get --json` is the recorded raw-value exception, SPEC-AS-022) | `serde_json` | Single choke point for output; JSON stability is a compatibility contract. |
| `credential` | trait `Provider { fn shallow_status(&self) -> Status; fn resolve(&self) -> Result<Secret>; fn store(&self, value) -> Result<()> }`; `Secret` newtype (no `Display`/`Debug` of contents) | The only module that touches secret values. Adapters: `EnvProvider`, `KeychainProvider` (keyring crate v4, default `v1` feature stores; mock via keyring-core `sample` in tests), `CommandProvider` (direct `argv` exec — the CLI never constructs a shell invocation; argv content is the config author's choice) | `keyring`, `std::process` | `Secret` redacts in Debug; `store` is `Err(Unsupported)` for env/command. Shallow status never resolves: env = getenv presence, keychain = "configured", command = argv[0] on PATH. SPEC-019 boundary: the command provider's inherited stderr/stdin belong to the external tool, outside the no-secret invariant. |
| `runner` | `InjectionPlan::build(&Config, &Profile, entries: &[name]) -> Result<InjectionPlan>`; `InjectionPlan::exec(cmd: &[String]) -> Error` (Unix: only returns on failure) | Recursively collect `credential://` refs (incl. `?as=` overrides), evaluate `inject` tables, detect env-name conflicts before resolving any secret, then resolve and launch | `credential`, `config` | Conflict detection precedes secret resolution so a failing plan never touches providers. Unix: `execvp` (process replacement gives exit-code/signal/stdio transparency for free). Windows: spawn + wait + code passthrough. |
| `cli` | `main()`, clap command tree, `Error -> exit code` mapping | Argument parsing, command dispatch, error rendering, exit codes 0/1/2/3/4/127, terminal interaction (no-echo secret prompt via `rpassword`, TTY-vs-pipe detection for `credential set`) | all above | The only module that prints to stderr/stdout for errors and the only module that reads from the terminal. |

## Data Model and State

| Entity / State | Owner | Lifecycle | Validation rules | Persistence |
| --- | --- | --- | --- | --- |
| `Config` (profiles, entries, credentials) | `config` | Loaded per invocation, immutable | Design doc §8 strict/open rules | User TOML file (read-only) |
| `Secret` | `credential` | Created during `run`/`credential check|set` only; dropped after use | Never serialized, never logged, no Debug/Display leak | System store / env / external command |
| `InjectionPlan` | `runner` | Per `run` invocation | Env-name uniqueness; valid env names; scalar-only inject sources | In-memory only |

## External Interfaces

| Interface | Consumer | Contract | Compatibility / versioning |
| --- | --- | --- | --- |
| CLI commands & exit codes | Agents, scripts, users | Design doc §5, §9 | Exit codes and JSON shapes are stable; additive JSON changes only |
| JSON output | Agents | Envelopes with top-level `version`, full paths, types, values (raw-value exception for `get --json`, SPEC-AS-022); credentials as name+provider+status only | Stable contract (PRD-NFR-003) |
| Config TOML schema | Users | Design doc §4, §8 | `version = 1`; unknown fields preserved |

## Decisions and Alternatives

### Decision ARCH-001: Rust with clap

- Decision: Implement in Rust; CLI framework `clap` (derive), single binary.
- Rationale: User-selected (session decision 2026-08-21). Millisecond startup suits per-turn agent calls; `keyring` crate covers all three platform credential stores; static binary removes runtime dependencies.
- Alternatives considered:
  - Python + `keyring`: fastest to write, rejected for interpreter startup latency and environment-dependent distribution.
  - TypeScript/Node: rejected — no maintained cross-platform keychain binding (keytar deprecated), largest startup overhead.
- Consequences: Slightly longer implementation time; tests via `cargo test` + `assert_cmd`.

### Decision ARCH-002: `toml` crate with `preserve_order`, generic value tree

- Decision: Parse with the `toml` crate (1.x; `preserve_order` feature verified available) into typed core structs (version, profiles, credentials) + `toml::Value` for open user fields. Credential store access via `keyring` 4.x (default `v1` feature enables apple-native, windows-native, and zbus-secret-service stores; tests use the keyring-core `sample` mock store).
- Rationale: `list`/`show` should present fields in file order (user's mental model); typed core gives strict validation; generic tail gives the open schema.
- Alternatives considered:
  - `toml_edit`: preserves comments/format — needed only for writing, which is out of scope. Rejected as heavier.
  - Fully typed schema: contradicts the open-schema requirement. Rejected.
- Consequences: Two-layer model (typed core + value tree) that `query` navigates generically.

### Decision ARCH-003: `run` uses process replacement (`execvp`) on Unix

- Decision: On Unix, `run` builds the injected environment and replaces itself with the target via `CommandExt::exec`. On Windows, spawn + wait + exit-code passthrough.
- Rationale: Process replacement makes stdio transparency, exit-code passthrough, signal delivery, and "no lingering wrapper" properties structural instead of implemented: the target *is* the process, so nothing can buffer, intercept, or misreport.
- Alternatives considered:
  - Spawn + wait + manual signal forwarding everywhere: more code, subtle bugs (signal races, exit-status translation), no benefit on Unix. Rejected for Unix, required on Windows.
- Consequences: `run` never returns on the Unix happy path; the design doc's `128 + signal` observable behavior is produced by the OS/shell, which satisfies the behavior contract.

### Decision ARCH-004: Provider seam as a trait with three production adapters

- Decision: `credential::Provider` trait; adapters `env`, `keychain` (keyring crate), `command`; plus keyring's mock store in tests.
- Rationale: Three genuinely different production adapters justify the seam; `run`, `credential check`, and `credential set` all consume it; tests substitute the keyring mock and fake commands.
- Alternatives considered:
  - Match-on-enum in each caller: duplicates dispatch in three call sites, loses the single choke point for the "shallow vs resolve" distinction. Rejected.
- Consequences: New providers (v2: e.g. OS-specific stores, sops) are additive.

### Decision ARCH-005: Secrets as a no-leak newtype confined to two modules

- Decision: `Secret(String)` with manual `Debug`/no `Display`/no `Serialize`; constructed by `credential` (provider resolution) and at the `cli` terminal-read boundary for `credential set` (the raw read is wrapped into `Secret` immediately, before any other call); consumed only by `runner` (env injection), `credential check` (discarded), and `Provider::store`. `query`/`render` types cannot represent a secret.
- Rationale: Makes PRD-NFR-001 a compile-time property rather than a code-review property.
- Alternatives considered:
  - Discipline-only (pass `String`s carefully): rejected — the whole point of the tool is that this class of leak must not depend on vigilance.
- Consequences: Small amount of newtype boilerplate; zeroize-on-drop noted as optional hardening (not required by spec).

## Risks and Mitigations

| Risk | Impact | Mitigation | Owner |
| --- | --- | --- | --- |
| Linux headless has no secret-service | `keychain` provider unusable there | Explicit error naming the provider and suggesting `command`/`env` (design doc §6.1); never a silent fallback | impl |
| keyring crate behavior differs per platform | Inconsistent `credential set/check` | Integration tests use keyring mock store; platform behavior documented in README | impl |
| JSON shape drift breaks agents | Agent workflows fail | JSON shapes defined in spec; snapshot tests lock them | impl |
| `command` provider hangs (e.g. `op` waiting for auth) | `run`/`check` blocks | Inherit stdin/stderr for the provider command so interactive auth is visible; document; no timeout in v1 (recorded assumption SPEC-AS-004) | impl |

## Testing Strategy

- Acceptance gates: spec acceptance criteria drive integration tests (`assert_cmd` against a temp config file + temp env).
- Automated tests: unit tests for `path` grammar, validation rules, injection-plan conflicts; integration tests for each CLI command including exit codes; fake scripts for `command` provider.
- Keychain test seam (SPEC-AS-019): a cargo feature `test-keychain`, enabled only for test builds, adds a file-backed store adapter selected via `AGENT_CONTEXT_TEST_KEYCHAIN=<path>`; release builds compile no test store. This keeps out-of-process `assert_cmd` tests honest while honoring the no-mock-in-production rule (the path exists only in test-feature builds). The keyring-core `mock` module additionally serves in-process unit tests of the adapter seam.
- Manual validation: real macOS Keychain round-trip (`credential set` → `check` → `run`) once, recorded in validation.md. Cross-OS builds/tests via CI matrix are a post-v1 follow-up recorded in validation.md risks.
- TDD seams: security-sensitive boundary trigger applies to two behaviors — (1) query/JSON output never contains resolved secret values, (2) `run` conflict detection precedes any provider resolution. Tests for these are authored before/independent of implementation per subagent-execution rules.

## Rollout, Migration, and Rollback

- Rollout: greenfield; `cargo install --path .` locally. No migration, no rollback concerns beyond git revert.

## Open Questions

| ID | Question | Impact if unresolved | Resolution |
| --- | --- | --- | --- |
| ARCH-Q-001 | none | — | — |

## Architecture Review Checklist

- [x] Module seams are explicit.
- [x] Interfaces include invariants, error modes, and important constraints.
- [x] Modules are deep; no pass-through module (deletion test applied to `render` — it owns the JSON stability contract and all redaction formatting, so it earns its seam).
- [x] Every port/seam has at least two justified adapters (`Provider`: 3 production + mock; `ConfigSource`: file + in-memory string for tests).
- [x] Test seam matches dependency category: keychain = true-external (mock adapter), command = local-substitutable (fake scripts), env = in-process.
- [x] Rejected alternatives documented.
- [x] Risks have mitigations.
- [x] Testing strategy matches risk and acceptance criteria.
- [x] No speculative architecture (no plugin system, no async runtime, no config-writing machinery).
