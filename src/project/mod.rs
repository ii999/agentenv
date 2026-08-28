//! Project-scoped configuration: discovery, closed-schema validation, and the
//! trust-on-first-use gate for a checked-in `.agentenv.toml` (change
//! 003-project-config). The facade composing these submodules into
//! `ProjectContext` lands with task T006.

pub mod locate;
pub mod model;
pub mod trust;
