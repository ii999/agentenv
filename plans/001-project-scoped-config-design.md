# Plan 001: Design project-scoped configuration and its pairing with .env / docker compose

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 3a7be3d..HEAD -- src/config/locate.rs src/runner.rs README.md`
> If any of these files changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

This is a **design plan**, not a build plan. The deliverable is one design
document, `docs/design/project-scoped-config.md`, good enough to serve as the
input for a future spec-driven change (the same role
`.sdd/changes/archive/2026-08-22-001-agent-context-cli/design-source.md`
played for the v1 CLI). **Do not modify any file under `src/`, `tests/`,
`skills/`, or the install scripts.**

## Status

- **Priority**: P1
- **Effort**: M (one focused day: research + writing)
- **Risk**: LOW (produces a document only)
- **Depends on**: none
- **Category**: direction
- **Planned at**: commit `3a7be3d`, 2026-08-28

## Why this matters

`agentenv` currently reads exactly one global, per-user config file, and the
active profile is chosen by `--profile`, `AGENTENV_PROFILE`, or the file's
`default_profile`. But the unit of agent work is a repository: an agent opens
a project and needs to know *which* profile applies there and *which* entries
the project expects (an LLM endpoint, a kubernetes context, CI tags). Today
that knowledge is conveyed by hand — the README tells projects to paste an
instruction block into `AGENTS.md`, and users must export `AGENTENV_PROFILE`
per shell or repeat `--profile` on every call. A checked-in project file that
pins a profile and declares required entries would make environment selection
deterministic per repository and give agents a discoverable answer to "what
does this project need from my environment?". The design must also settle how
this pairs with the project-scoped mechanisms that already exist in the wild —
`.env` files, docker compose, direnv — because that is where users will
actually feel the feature.

## Current state

Relevant files (read all of them before writing):

- `src/config/locate.rs` — resolves the single config file path; pure
  environment logic. Lines 20–31:

  ```rust
  pub fn resolve_path(
      explicit_file: Option<&Path>,
      env: &impl Fn(&str) -> Option<String>,
  ) -> Result<PathBuf, AppError> {
      if let Some(path) = explicit_file {
          return Ok(path.to_path_buf());
      }
      if let Some(file) = env_value(env, "AGENTENV_FILE") {
          return Ok(PathBuf::from(file));
      }
      default_path(env)
  }
  ```

  There is no notion of a project file or of walking up from the working
  directory. On Unix the loaded file's permission bits must be a subset of
  `0600` (see README "Configuration" section) — a checked-in project file
  cannot meet that rule, so the design must give the project file its own
  validation story.

- `src/runner.rs` — `run` builds the child environment from the full
  inherited environment plus planned injections (lines 135–154) and launches
  the target transparently (`env_clear().envs(environment)`, line 225). Any
  compose/`.env` pairing pattern must work through this mechanism, because it
  is the only path by which a credential value ever reaches another process.

- `README.md` — the behavior contract. Load-bearing sections:
  - Profile precedence (lines 97–101): `--profile`, then `AGENTENV_PROFILE`,
    then `default_profile`.
  - Agent usage protocol and the hand-pasted `AGENTS.md` block (lines
    174–200) — the friction this feature replaces.
  - Safety and threat model (lines 354–396), especially the paragraph
    explaining that write commands let any caller rewrite injection topology
    and that mitigations are whole-file validation, no secret values in TOML,
    and visible-in-file review.

- `.sdd/changes/archive/2026-08-22-001-agent-context-cli/prd.md` — recorded
  non-goals (lines 23–26): no GUI, no cloud sync, no automatic config
  editing, no guessing missing fields, never printing plaintext credentials,
  and no protection against a malicious agent that deliberately reads
  injected variables from a process it launches. The design must not
  contradict these; where it touches them, cite them.

- `.sdd/changes/archive/2026-08-22-001-agent-context-cli/design-source.md` —
  the v1 design document (Chinese). Its structure (problem, format, commands,
  exit codes, threat model, version scope) is the structural model for the
  document you will write. Write the new document in English.

Repo conventions that apply:

- Documents state behavior contracts precisely, including exit codes, error
  message obligations, and explicit "what this does NOT do" sections — match
  the README's register.
- Per the user's global standards, avoid silent fallbacks anywhere in the
  designed behavior: a missing or invalid project file must produce an
  explicit, defined outcome, never a quiet skip.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Build   | `cargo build` | exit 0 |
| Tests   | `cargo test`  | all pass (baseline sanity only; you change no code) |
| Doc structure check | `grep -c '^## ' docs/design/project-scoped-config.md` | ≥ 10 |

## Scope

**In scope** (the only files you may create or modify):

- `docs/design/project-scoped-config.md` (create, along with the `docs/design/` directory)
- `plans/README.md` (status row update only)

**Out of scope** (do NOT touch):

- Everything under `src/`, `tests/`, `skills/` — this plan produces no code.
- `README.md` — it documents shipped behavior only; the design doc is not shipped behavior.
- `.sdd/` — managed by the SDD workflow, not by this plan.

## Git workflow

- Branch: `advisor/001-project-scoped-config-design`
- Single commit is fine; message style matches repo history (imperative,
  no prefix), e.g. `Add project-scoped configuration design document`.
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Absorb the existing contracts

Read, in this order: `README.md` in full; `src/config/locate.rs`;
`src/runner.rs`; the PRD non-goals cited above. Extract into your notes:
the profile-precedence chain, the exit-code table, the no-secret invariant
sentences, and the Unix `0600` permission rule. Every design decision you
write must be checked against these.

**Verify**: you can state, without reopening the files, why a checked-in
project file cannot be loaded through the same path as the user config
(permission rule + attacker-supplied content). Write that statement into your
notes; it becomes part of the doc's threat-model section.

### Step 2: Survey prior art for project-scoped trust

Research how existing tools gate repo-supplied configuration, using their
official docs (WebFetch or local knowledge):

- **direnv**: `.envrc` is inert until `direnv allow`; re-approval required on
  every content change (hash-based). This is the reference trust model.
- **mise** (`mise.toml` / `mise trust`): same trust-on-first-use pattern.
- **docker compose**: `${VAR}` interpolation in `compose.yaml` reads the
  parent process environment and a plaintext `.env` in the project directory;
  `environment:` entries with no value pass the variable through from the
  parent environment; `env_file:` loads plaintext files into containers.
- **1Password Environments**: `.env` backed by a named pipe so plaintext
  never lands on disk — relevant as future work, not v1.

Record for each: what the project file may contain, how trust is established,
and what happens when trust is absent.

**Verify**: your notes contain a trust-model row for at least direnv, mise,
and compose.

### Step 3: Draft the design document

Create `docs/design/project-scoped-config.md` with at least these `##`
sections, each making a concrete decision (or explicitly deferring it to the
open-questions section):

1. **Problem** — per-repo determinism; the hand-pasted `AGENTS.md` block;
   `AGENTENV_PROFILE` friction. Ground it with the current README protocol.
2. **Proposal overview** — a checked-in project file (working name
   `.agentenv.toml`) discovered from the working directory.
3. **File discovery** — exact walk-up rules: start at CWD, walk parent
   directories, stop at the filesystem root and do not cross `$HOME`'s
   parent; first file wins. Define interaction with `AGENTENV_FILE`
   (which selects the *user* config, not the project file) and a
   `--no-project` / `AGENTENV_NO_PROJECT` escape hatch.
4. **Allowed content — the narrow waist.** Recommended v1 scope, and the
   rationale for its narrowness: the project file may only *select and
   declare*, never *define*:
   - `profile = "<name>"` — pins the active profile (inserted into the
     precedence chain; decide exactly where — recommended: below `--profile`
     and `AGENTENV_PROFILE`, above `default_profile`, so explicit user intent
     always wins).
   - `[requires]` — a table declaring required entries and (optionally)
     required fields, e.g. `entries = ["llm", "kubernetes"]`, each with a
     human/agent-readable reason. Surfaced by a new read command (candidate:
     `agentenv project status --json`) that reports which requirements the
     user's config satisfies — an explicit report, never a silent skip.
   - Explicitly forbidden in v1: credential definitions, `inject` tables,
     `?as=` overrides, entry values of any kind. State why: a repo file that
     could shape injection topology would let a cloned repository redirect
     which secret lands in which environment variable — exactly the risk the
     README's threat-model paragraph confines to the user-owned file today.
5. **Trust model** — decide between (a) inert-until-allowed
   (direnv-style `agentenv project allow`, content-hash recorded in the
   *user* config or a state file; changed file ⇒ re-approval) and
   (b) always-honored-because-inert (the v1 content above only selects
   among things the user already defined, so honoring it without approval
   may be acceptable). Present both, recommend one, and enumerate what each
   would mean for the exit-code table. If (b) is chosen, spell out the
   residual risk: a repo can pin a profile the user did not intend
   (e.g. switch `personal` → `work`), which selects different credentials.
6. **Pairing with `.env` files** — the decision the user specifically asked
   for. Cover:
   - Why `agentenv` must never *generate* a `.env` containing credential
     values (no-secret invariant; secrets never land in plaintext on disk).
   - The supported pattern: `.env` keeps non-secret, project-local values
     (ports, feature flags); credentials arrive only through
     `agentenv run --with ... -- <tool>` into the process environment.
   - Whether v1 should include `agentenv project env` to *generate a
     non-secret* `.env` fragment from ordinary entry fields (recommended:
     defer; record as future work with the FIFO-backed approach).
7. **Pairing with docker compose** — the canonical workflow to document:
   `agentenv run --with llm -- docker compose up` with `compose.yaml`
   using `${OPENAI_API_KEY}` interpolation or bare `environment: - OPENAI_API_KEY`
   passthrough; containers receive the value from compose's own process
   environment, which `run` populated. State plainly that `env_file:` with
   secrets is the anti-pattern this replaces. Include a worked example
   (compose snippet + the matching `.agentenv.toml` + the command line).
8. **Pairing with direnv** — honest comparison: profile pinning alone is
   achievable today with `.envrc` exporting `AGENTENV_PROFILE` (a non-secret
   export, fully compatible). The project file must therefore justify itself
   beyond pinning — the `[requires]` declaration and agent discoverability
   are that justification. Say so explicitly; if during writing you conclude
   the justification is too thin, that is a finding to surface, not to bury.
9. **CLI surface changes** — new commands/flags with exact names, JSON
   output shapes, and exit codes consistent with the existing table
   (0/1/2/3/4/127); which existing commands change behavior (at minimum:
   everything that resolves a profile).
10. **Skill and protocol impact** — what changes in `skills/agentenv/SKILL.md`
    and the README `AGENTS.md` block once this ships (describe; do not edit
    those files).
11. **Threat model delta** — new attack surface (attacker-supplied repo
    file), mitigations, and what remains explicitly out of scope (malicious
    agents reading injected env — already a recorded non-goal).
12. **Open questions** — every deferred decision, each with a recommended
    answer and the reason it needs the maintainer's call.

**Verify**: `grep -c '^## ' docs/design/project-scoped-config.md` → ≥ 10;
every section above maps to a heading.

### Step 4: Self-check against the invariants

Re-read the finished document once against this checklist:

- No sentence permits a credential value to reach disk, stdout, stderr, or
  the project file.
- Every failure mode (missing project file, unparseable file, unknown pinned
  profile, unsatisfied requirement, untrusted file) has a defined, explicit
  outcome and exit code — no silent skip, no silent fallback.
- Nothing contradicts the recorded non-goals; where a decision touches one,
  the non-goal is cited.
- The compose worked example uses passthrough/interpolation, never a
  secret-bearing `env_file:`.

**Verify**: `grep -in "env_file" docs/design/project-scoped-config.md` —
every match is in a sentence advising against it for secrets.

## Test plan

No code changes, so no new tests. Baseline sanity only: `cargo test` passes
before and after (your change touches no Rust source; a failure means
pre-existing drift — report it, don't fix it).

## Done criteria

ALL must hold:

- [ ] `docs/design/project-scoped-config.md` exists with all 12 sections
- [ ] `grep -c '^## ' docs/design/project-scoped-config.md` ≥ 10
- [ ] `git status --porcelain` shows only `docs/design/project-scoped-config.md` and `plans/README.md`
- [ ] `cargo test` exits 0
- [ ] Open-questions section: every entry has a recommendation
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- The drift check shows `src/config/locate.rs`, `src/runner.rs`, or the
  README profile/threat-model sections changed since commit `3a7be3d`.
- You conclude the feature requires the project file to define credentials or
  `inject` mappings to be useful — that contradicts the threat-model stance
  this plan mandates; the maintainer must decide, not you.
- You find an existing mechanism in the codebase for project-level
  configuration this plan is unaware of.

## Maintenance notes

- This document is the input for a future SDD change (likely
  `003-project-config`); its decisions become the spec's requirements. Keep
  wording contract-grade.
- A reviewer should scrutinize section 5 (trust model) hardest — it is the
  only section with a security decision — and section 8's honesty about the
  direnv overlap.
- Deliberately deferred: non-secret `.env` generation, FIFO-backed `.env`,
  and any MCP exposure of project status (separate direction finding).
