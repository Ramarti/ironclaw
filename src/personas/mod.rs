//! Agent persona system for role-based access control.
//!
//! Personas are TOML files that define named roles (designer, developer,
//! researcher, etc.) with structural restrictions on tools, shell commands,
//! sandbox policy, and system prompt identity.
//!
//! Discovery locations:
//! - `~/.ironclaw/personas/` — user-global personas
//! - `<workspace>/personas/` — per-workspace personas (win on name collision)

mod enforcement;
mod loader;
mod registry;
mod types;

pub use enforcement::{PersonaFilterResult, filter_tools_for_persona, matches_command_pattern};
pub use loader::load_persona;
pub use registry::PersonaRegistry;
pub use types::{LoadedPersona, PersonaConfig};

/// Errors related to persona loading and validation.
#[derive(Debug, thiserror::Error)]
pub enum PersonaError {
    #[error("Persona not found: {name}")]
    NotFound { name: String },

    #[error("Failed to read persona file {path}: {reason}")]
    ReadError { path: String, reason: String },

    #[error("Failed to parse persona file {path}: {reason}")]
    ParseError { path: String, reason: String },

    #[error("Persona validation failed for {name}: {reason}")]
    ValidationError { name: String, reason: String },
}
