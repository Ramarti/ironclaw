//! Persona configuration types deserialized from TOML files.

use std::path::PathBuf;

use serde::Deserialize;

use crate::sandbox::SandboxPolicy;

/// Top-level persona configuration matching the TOML schema.
#[derive(Debug, Clone, Deserialize)]
pub struct PersonaConfig {
    /// Core persona metadata.
    pub persona: PersonaMeta,

    /// Prompt/identity configuration.
    #[serde(default)]
    pub prompt: PersonaPrompt,

    /// Tool access restrictions.
    #[serde(default)]
    pub tools: PersonaToolAccess,

    /// Shell command restrictions.
    #[serde(default)]
    pub shell: PersonaShellAccess,

    /// Sandbox policy override.
    #[serde(default)]
    pub sandbox: PersonaSandbox,
}

/// Core persona metadata.
#[derive(Debug, Clone, Deserialize)]
pub struct PersonaMeta {
    /// Unique persona name (must match filename stem).
    pub name: String,

    /// Human-readable description.
    #[serde(default)]
    pub description: String,
}

/// Prompt/identity injection configuration.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PersonaPrompt {
    /// Inline identity text injected as system prompt prefix.
    pub identity: Option<String>,

    /// Path to identity file, resolved relative to the TOML parent directory.
    pub identity_file: Option<String>,
}

/// Tool access control (allowlist or denylist, mutually exclusive).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PersonaToolAccess {
    /// Allowlist: only these tools are available.
    #[serde(default)]
    pub allow: Vec<String>,

    /// Denylist: all tools except these are available.
    #[serde(default)]
    pub deny: Vec<String>,
}

impl PersonaToolAccess {
    /// Returns true if no tool restrictions are configured.
    pub fn is_unrestricted(&self) -> bool {
        self.allow.is_empty() && self.deny.is_empty()
    }
}

/// Shell command access control.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PersonaShellAccess {
    /// Glob patterns for permitted commands. Empty = no shell access.
    #[serde(default)]
    pub allowed_commands: Vec<String>,
}

/// Sandbox policy override from persona configuration.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PersonaSandbox {
    /// Sandbox policy override. None = use system default.
    #[serde(default, deserialize_with = "deserialize_sandbox_policy")]
    pub policy: Option<SandboxPolicy>,
}

/// A fully resolved persona ready for use.
#[derive(Debug, Clone)]
pub struct LoadedPersona {
    /// Parsed configuration from the TOML file.
    pub config: PersonaConfig,

    /// Resolved identity text (from inline or file).
    pub identity_text: Option<String>,

    /// Path to the source TOML file.
    pub source_path: PathBuf,
}

impl LoadedPersona {
    /// The persona name.
    pub fn name(&self) -> &str {
        &self.config.persona.name
    }

    /// The persona description.
    pub fn description(&self) -> &str {
        &self.config.persona.description
    }
}

/// Custom deserializer for `Option<SandboxPolicy>` from a string.
fn deserialize_sandbox_policy<'de, D>(deserializer: D) -> Result<Option<SandboxPolicy>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt: Option<String> = Option::deserialize(deserializer)?;
    match opt {
        None => Ok(None),
        Some(s) => s
            .parse::<SandboxPolicy>()
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_minimal_persona() {
        let toml_str = r#"
[persona]
name = "designer"

[prompt]
identity = "You are a designer."
"#;
        let config: PersonaConfig = toml::from_str(toml_str).expect("parse");
        assert_eq!(config.persona.name, "designer");
        assert_eq!(
            config.prompt.identity.as_deref(),
            Some("You are a designer.")
        );
        assert!(config.tools.is_unrestricted());
        assert!(config.shell.allowed_commands.is_empty());
        assert!(config.sandbox.policy.is_none());
    }

    #[test]
    fn test_deserialize_full_persona() {
        let toml_str = r#"
[persona]
name = "researcher"
description = "Research specialist"

[prompt]
identity = "You are a researcher."

[tools]
allow = ["memory_search", "http"]

[shell]
allowed_commands = ["curl *", "wget *"]

[sandbox]
policy = "ReadOnly"
"#;
        let config: PersonaConfig = toml::from_str(toml_str).expect("parse");
        assert_eq!(config.persona.name, "researcher");
        assert_eq!(config.persona.description, "Research specialist");
        assert_eq!(config.tools.allow, vec!["memory_search", "http"]);
        assert!(config.tools.deny.is_empty());
        assert_eq!(config.shell.allowed_commands, vec!["curl *", "wget *"]);
        assert_eq!(config.sandbox.policy, Some(SandboxPolicy::ReadOnly));
    }

    #[test]
    fn test_deserialize_deny_mode() {
        let toml_str = r#"
[persona]
name = "safe"

[tools]
deny = ["shell", "write_file"]
"#;
        let config: PersonaConfig = toml::from_str(toml_str).expect("parse");
        assert!(config.tools.allow.is_empty());
        assert_eq!(config.tools.deny, vec!["shell", "write_file"]);
        assert!(!config.tools.is_unrestricted());
    }

    #[test]
    fn test_deserialize_identity_file() {
        let toml_str = r#"
[persona]
name = "dev"

[prompt]
identity_file = "dev/IDENTITY.md"
"#;
        let config: PersonaConfig = toml::from_str(toml_str).expect("parse");
        assert!(config.prompt.identity.is_none());
        assert_eq!(
            config.prompt.identity_file.as_deref(),
            Some("dev/IDENTITY.md")
        );
    }

    #[test]
    fn test_sandbox_policy_variants() {
        for (input, expected) in [
            ("ReadOnly", SandboxPolicy::ReadOnly),
            ("WorkspaceWrite", SandboxPolicy::WorkspaceWrite),
            ("FullAccess", SandboxPolicy::FullAccess),
        ] {
            let toml_str = format!("[persona]\nname = \"t\"\n[sandbox]\npolicy = \"{}\"", input);
            let config: PersonaConfig = toml::from_str(&toml_str).expect("parse");
            assert_eq!(config.sandbox.policy, Some(expected));
        }
    }

    #[test]
    fn test_tool_access_unrestricted() {
        let access = PersonaToolAccess::default();
        assert!(access.is_unrestricted());
    }
}
