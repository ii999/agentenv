# Product Requirements Document: agent-context CLI

Backfilled summary. The authoritative product description is the user-approved design document preserved verbatim at `design-source.md` (Chinese). This PRD condenses it for traceability; where wording differs, `design-source.md` governs.

## Source

- Change ID: 001-agent-context-cli
- Input source: imported design document (`design-source.md`), user-reviewed
- Authoring date: 2026-08-21
- Owner: zhaiqifeng

## Problem

Local coding agents repeatedly need user-environment facts — LLM endpoints, CI tags, cluster names, cache addresses — and the credentials that go with them. Today this knowledge lives in scattered shell profiles, notes, and ad-hoc environment variables. Agents either guess, ask the user every session, or worse, end up with plaintext secrets in transcripts and logs.

## Goals

- One human-editable TOML file holds all ordinary configuration, grouped by profile (work, personal, ...), extensible without CLI code changes.
- A single CLI (`agent-context`) lets agents discover, search, and read that configuration in text or stable JSON.
- Secrets never enter the config file, CLI query output, error messages, or logs; they are stored in the system credential store, environment variables, or an external password manager, and injected only into a target process launched via `agent-context run`.
- Missing configuration or unavailable credentials produce explicit, actionable errors with distinct exit codes.

## Non-Goals

- GUI, cloud sync, automatic editing of the config file, guessing missing fields, or ever printing plaintext credentials (per design doc §11).
- Protection against a malicious agent that deliberately reads injected environment variables from a process it launches: the tool prevents accidental leakage only (design doc §10 threat model).

## Users and Use Cases

| User / Actor | Need | Current pain | Desired outcome |
| --- | --- | --- | --- |
| Coding agent (CLI-driven) | Discover and read user environment facts; run tools that need credentials | Guessing, asking, or leaking secrets | `list`/`show`/`get`/`find` + `run --with` with zero plaintext exposure |
| Developer (human) | Maintain one readable config; store secrets safely | Scattered dotfiles and pasted tokens | Edit one TOML file; `credential set`/`check` manage the secret store |

## Functional Requirements

- PRD-FR-001: The user MUST be able to define arbitrary profiles and configuration entries with arbitrary TOML fields, each carrying a mandatory description, without modifying the CLI.
- PRD-FR-002: The CLI MUST resolve the active profile from `--profile`, then `AGENT_CONTEXT_PROFILE`, then `default_profile`, and fail with the available options listed when none applies.
- PRD-FR-003: The user and agent MUST be able to browse (`list`), inspect (`show`), read (`get`), and search (`find`) configuration in text and stable JSON.
- PRD-FR-004: Configuration MUST be able to reference credentials (`credential://name`, optional `?as=ENV`) stored via `env`, `keychain`, or `command` providers; queries return only the reference and a status, never the value.
- PRD-FR-005: `agent-context run --with <entry> -- <cmd>` MUST launch the target command with referenced credentials and declared `inject`-table values injected as environment variables, with conflict detection and transparent process semantics.
- PRD-FR-006: `validate` MUST check structure, references, descriptions, suspected plaintext secrets, and (on Unix) file permissions, failing with a non-zero exit code and the offending config path.
- PRD-FR-007: The user MUST be able to manage stored secrets with `credential list`, `credential check <name>`, and `credential set <name>` (keychain provider only for `set`).

## Non-Functional Requirements

- PRD-NFR-001 (security): No CLI query command, error message, or log ever contains a credential value; browse commands perform only shallow credential status checks with no side effects (no prompts, no network, no secret reads).
- PRD-NFR-002 (predictability): Every failure mode has a defined exit code (design doc §9) and an error message naming the next action; no silent fallback anywhere.
- PRD-NFR-003 (agent ergonomics): JSON output is stable across releases and includes the config `version`; the CLI is fast enough for per-turn agent invocation (interactive latency, no perceptible startup delay).

## Constraints

- Config format is TOML (user preference and design doc §3).
- Platforms: macOS, Linux, Windows; system credential store per platform (Keychain, secret-service, Credential Manager).
- The design document (as updated 2026-08-21) is the binding behavior contract.

## Assumptions

- Single-user, local-machine usage; no concurrent-writer protection needed beyond normal file reads.

## Acceptance Summary

The product requirement is accepted when:

- A new entry or field added to the TOML file appears in `list`/`show`/`get`/`find` with no CLI change.
- An agent can complete the documented workflow (discover → read → `run --with`) without any plaintext credential appearing in its transcript.
- All error paths return the documented exit codes and messages.

## Traceability Seeds

| PRD ID | Expected downstream artifact |
| --- | --- |
| PRD-FR-001..007 | SPEC requirements (spec.md) |
| PRD-NFR-001 | ARCH-005, SPEC security requirements |
| PRD-NFR-002 | SPEC exit-code requirements |
| PRD-NFR-003 | ARCH-001, SPEC JSON requirements |
