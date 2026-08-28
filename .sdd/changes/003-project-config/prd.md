# Product Requirements Document: project-config

## Source

- Change ID: 003-project-config
- Input source: direction audit and design plan (`plans/001-project-scoped-config-design.md`), plus user interview in this change
- Authoring date: 2026-08-28
- Owner: zhaiqifeng

## Problem

`agentenv` resolves exactly one global, per-user configuration file, and the active profile comes from `--profile`, `AGENTENV_PROFILE`, or the file's `default_profile`. The unit of agent work, however, is a repository: an agent opening a project must learn which profile applies there and which entries the project expects (an LLM endpoint, a kubernetes context, CI tags). Today that knowledge travels by hand — a pasted instruction block in `AGENTS.md`, or a per-shell `AGENTENV_PROFILE` export — so agents guess, ask, or silently use the wrong profile, and a project has no machine-readable way to state its environment needs.

## Goals

- A project can carry a checked-in file that pins the agentenv profile to use inside that project tree.
- A project can declare which configuration entries (and optionally fields) it requires, with human/agent-readable reasons, and agents/users can query whether the active user configuration satisfies those requirements.
- A checked-in file never takes effect without the user's explicit, revocable approval of its exact content.
- The existing no-secret invariant is fully preserved: the project file can never introduce, redefine, or reroute credentials, and no command prints a credential value.

## Non-Goals

- Project files defining configuration values, credential definitions, `inject` tables, or `?as=` overrides — the project file selects and declares only (interview decision D-01).
- Generating `.env` files from configuration, secret-bearing or otherwise; pairing with `.env`/docker compose is documentation of the existing `run` workflow, not a new value-emission mechanism.
- Cloud sync, GUI, or guessing missing fields (standing non-goals from change 001).
- Protection against a malicious agent that deliberately reads injected environment variables from a launched process (standing threat-model boundary from change 001).

## Users and Use Cases

| User / Actor | Need | Current pain | Desired outcome |
| --- | --- | --- | --- |
| Coding agent (CLI-driven) | Know which profile and entries apply to the repo it is working in | Guessing, asking the user, or reading hand-pasted AGENTS.md prose | Deterministic profile selection and a queryable requirements report inside the project tree |
| Developer (human) | Make a repo's environment needs explicit for collaborators and agents | Tribal knowledge, per-shell exports, README prose | One small checked-in TOML file, honored only after explicit approval |
| Developer (human, security-conscious) | Ensure a cloned repo cannot steer credential selection | No project-level mechanism exists at all today | Untrusted project files are inert until approved; any content change requires re-approval |

## User Stories

### PRD-US-001: Pin a profile per project (Priority: P1)

As a developer, I want a checked-in project file that pins the agentenv profile for that repository, so that every agent session inside the repo resolves the same profile without per-shell setup.

- Why this priority: deterministic profile selection is the core value; everything else builds on the file existing and being honored.
- Independent test: in a trusted project tree whose file pins profile `work`, read commands resolve `work` with no `--profile` flag and no `AGENTENV_PROFILE`; outside the tree, resolution falls back to `default_profile`.

### PRD-US-002: Approve or revoke a project file (Priority: P1)

As a developer, I want project files to be inert until I explicitly approve their exact content, so that cloning a repository can never silently change which profile — and therefore which credentials — my agent uses.

- Why this priority: the trust gate is a security precondition of US-001, not an add-on; the two ship together.
- Independent test: in a fresh clone, the pin has no effect and commands report the untrusted file; after the approval command, the pin takes effect; after editing the file, it is inert again until re-approved.

### PRD-US-003: Declare and check project requirements (Priority: P2)

As a developer or agent, I want the project file to declare required entries and fields with reasons, and a command that reports which requirements the active configuration satisfies, so that missing environment setup is discovered explicitly instead of failing mid-task.

- Why this priority: valuable discoverability, but meaningful only once the file exists and the trust gate works.
- Independent test: with a trusted file requiring entries `llm` and `kubernetes`, the status command reports satisfied/missing per requirement in text and JSON against user configs that do and do not define them.

## Functional Requirements

- PRD-FR-001: A project file discovered from the working directory MUST be able to pin the active profile, taking effect only within that project tree.
- PRD-FR-002: Profile selection precedence MUST be `--profile`, then `AGENTENV_PROFILE`, then the trusted project pin, then `default_profile` (interview decision D-03); explicit user intent always wins over the repo file.
- PRD-FR-003: The project file MUST support declaring required entries, and optionally required fields within entries, each with a reason; a command MUST report each requirement as satisfied or unsatisfied against the active profile, in text and JSON.
- PRD-FR-004: A project file MUST have no effect of any kind until the user approves its exact content; any change to an approved file MUST return it to the inert state until re-approved (interview decision D-02). Approval state is user-owned and never stored in the repository.
- PRD-FR-005: The user MUST be able to approve, re-approve, revoke, and inspect the trust state of a project file through the CLI.
- PRD-FR-006: The project file MUST be restricted to selection and declaration content; any value definition, credential definition, `inject` table, or credential reference in it MUST fail validation with an explicit error (interview decision D-01).
- PRD-FR-007: Every command that resolves configuration MUST behave identically to today when no project file is present, and MUST report — never silently ignore — a present-but-untrusted or invalid project file.
- PRD-FR-008: The user and agent MUST be able to bypass project-file discovery explicitly for a single invocation or a session.

## Non-Functional Requirements

- PRD-NFR-001 (security): The project file cannot cause any credential value to be printed, persisted, or rerouted to a different environment-variable target; the no-secret invariant of the existing CLI is unchanged.
- PRD-NFR-002 (predictability): Every new failure mode (unparseable project file, forbidden content, unknown pinned profile, untrusted file, unsatisfied requirement) has a defined exit status and an error message naming the next action; no silent fallback anywhere.
- PRD-NFR-003 (agent ergonomics): New JSON output is stable, and project-file discovery adds no perceptible startup latency to per-turn agent invocation.
- PRD-NFR-004 (compatibility): Existing configurations, command-line surfaces, and exit statuses continue to work unchanged in the absence of a project file.

## Data and Entities

| Entity | Description | Key attributes | Notes |
| --- | --- | --- | --- |
| Project file | Checked-in TOML discovered from the working directory | profile pin; requirement declarations (entries, optional fields, reasons) | Selection/declaration only; no values, credentials, or inject content |
| Trust record | User-owned approval of one project file's exact content | file identity; content fingerprint; approval time | Never stored in the repository; any content change invalidates it |
| Requirement report | Result of checking declarations against the active profile | per-requirement satisfied/unsatisfied and reason | Text and JSON output |

## UX and Interaction Notes

- Discovery walks from the working directory upward; the exact stopping rules are an architecture decision, but behavior must be deterministic and documented.
- An untrusted or invalid project file is surfaced as an explicit notice or error — commands never pretend the file is absent.
- Approval is a deliberate, single-purpose command interaction (direnv `allow` / mise `trust` pattern); its output names the file and what approval will cause.
- The agent skill and README protocol gain a project-discovery step; the pairing with `.env` files and docker compose is documented as: non-secret values may live in `.env`, credentials reach tools only through `agentenv run --with … -- <command>` (e.g. `docker compose up` with variable passthrough/interpolation), and `env_file:`-with-secrets is the anti-pattern this replaces.

## Success Metrics

- PRD-SM-001: In a trusted project, an agent following the documented protocol resolves the intended profile and learns all declared requirements without asking the user or reading prose instructions.
- PRD-SM-002: A user cloning a repository with a project file observes zero behavior change in every agentenv command until they run the approval command.
- PRD-SM-003: All pre-existing commands behave byte-identically in trees without a project file.

## Constraints

- Config format is TOML (project standard).
- Platforms: macOS, Linux, Windows — discovery and trust state must work on all three.
- The user-owned config file keeps its Unix `0600` permission rule; the checked-in project file necessarily has its own, different validation story.
- Exit statuses extend the existing documented table without changing existing meanings.

## Assumptions

- The project file name is `.agentenv.toml` at a directory level within the project tree (exact name and walk-up bounds settled in architecture; assumption safe because discovery is fully specified before implementation).
- Trust state lives in a user-owned state location outside the repository (exact location settled in architecture).
- Requirement checking is a read-only report; it does not block other commands (a missing requirement is surfaced by the status command, not by failing unrelated reads).
- The `[requires]` reason text is plain description, following the existing mandatory-description convention.

## Open Questions

| ID | Question | Owner | Resolution |
| --- | --- | --- | --- |
| PRD-Q-001 | Exact project file name and discovery stopping rules | Architecture | Open — settle in architecture.md |
| PRD-Q-002 | Trust-state storage location and fingerprint mechanism | Architecture | Open — settle in architecture.md |
| PRD-Q-003 | CLI surface naming (`agentenv project status/allow/…`) and exit-code assignments | Architecture | Open — settle in architecture.md |

## Acceptance Summary

The product requirement is accepted when:

- A trusted project file pins profile selection per PRD-FR-002 and the requirement report works in text and JSON per PRD-FR-003.
- An untrusted, edited, or revoked project file is fully inert and explicitly reported, per PRD-FR-004/FR-007.
- Forbidden project-file content fails validation with an explicit error per PRD-FR-006.
- All pre-existing behavior is unchanged when no project file is present per PRD-NFR-004.

## Traceability Seeds

| PRD ID | Expected downstream artifact |
| --- | --- |
| PRD-FR-001..008 | SPEC requirements (spec.md) |
| PRD-NFR-001 | ARCH threat-model delta; SPEC security criteria |
| PRD-NFR-002 | SPEC exit-status requirements |
| PRD-NFR-003 | SPEC JSON stability criteria |
| PRD-NFR-004 | SPEC compatibility criteria |

## Clarifications

Interview decisions recorded 2026-08-28:

- D-01 (file scope): Selection-only — profile pin plus `[requires]` declarations; the project file defines nothing. Options considered: selection-only (chosen, recommended), selection plus non-secret value overlay, pin-only.
- D-02 (trust model): Trust-on-first-use — inert until explicit approval of exact content; any change requires re-approval; state is user-owned. Options considered: trust-on-first-use (chosen, recommended), always honored, honored-except-pin.
- D-03 (precedence): `--profile` > `AGENTENV_PROFILE` > project pin > `default_profile`. Options considered: pin below env var (chosen, recommended), pin above env var.
