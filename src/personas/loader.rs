//! Persona file discovery and TOML parsing.

use std::path::Path;

use crate::personas::PersonaError;
use crate::personas::types::{LoadedPersona, PersonaConfig};

/// Maximum persona file size (64 KiB, consistent with skills).
const MAX_FILE_SIZE: u64 = 64 * 1024;

/// Regex pattern for valid persona names.
const NAME_PATTERN: &str = r"^[a-zA-Z0-9][a-zA-Z0-9._-]{0,63}$";

/// Discover all persona files in a directory.
///
/// Scans for `*.toml` files, parses and validates each one.
/// Returns successfully loaded personas; errors are logged but not fatal.
pub async fn discover_from_dir(dir: &Path) -> Vec<LoadedPersona> {
    let mut personas = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            tracing::debug!("Cannot read personas directory {}: {}", dir.display(), e);
            return personas;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }

        match load_persona(&path) {
            Ok(persona) => {
                tracing::debug!(
                    "Loaded persona '{}' from {}",
                    persona.name(),
                    path.display()
                );
                personas.push(persona);
            }
            Err(e) => {
                tracing::warn!("Failed to load persona from {}: {}", path.display(), e);
            }
        }
    }

    personas
}

/// Load and validate a single persona TOML file.
pub fn load_persona(path: &Path) -> Result<LoadedPersona, PersonaError> {
    let path_str = path.display().to_string();

    // Reject symlinks (consistent with skills)
    let metadata = std::fs::symlink_metadata(path).map_err(|e| PersonaError::ReadError {
        path: path_str.clone(),
        reason: e.to_string(),
    })?;

    if metadata.file_type().is_symlink() {
        return Err(PersonaError::ValidationError {
            name: path_str.clone(),
            reason: "Symlinks are not allowed for persona files".to_string(),
        });
    }

    // Check file size
    if metadata.len() > MAX_FILE_SIZE {
        return Err(PersonaError::ValidationError {
            name: path_str.clone(),
            reason: format!(
                "File size {} exceeds maximum {} bytes",
                metadata.len(),
                MAX_FILE_SIZE
            ),
        });
    }

    // Read file
    let content = std::fs::read_to_string(path).map_err(|e| PersonaError::ReadError {
        path: path_str.clone(),
        reason: e.to_string(),
    })?;

    // Parse TOML
    let config: PersonaConfig = toml::from_str(&content).map_err(|e| PersonaError::ParseError {
        path: path_str.clone(),
        reason: e.to_string(),
    })?;

    // Validate name matches filename stem
    let expected_name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    if config.persona.name != expected_name {
        return Err(PersonaError::ValidationError {
            name: config.persona.name.clone(),
            reason: format!(
                "Persona name '{}' must match filename stem '{}'",
                config.persona.name, expected_name
            ),
        });
    }

    // Validate name format
    let name_re = regex::Regex::new(NAME_PATTERN).expect("valid regex");
    if !name_re.is_match(&config.persona.name) {
        return Err(PersonaError::ValidationError {
            name: config.persona.name.clone(),
            reason: format!("Name must match pattern {}", NAME_PATTERN),
        });
    }

    // Validate allow/deny mutual exclusivity
    if !config.tools.allow.is_empty() && !config.tools.deny.is_empty() {
        return Err(PersonaError::ValidationError {
            name: config.persona.name.clone(),
            reason: "tools.allow and tools.deny are mutually exclusive".to_string(),
        });
    }

    // Resolve identity text
    let identity_text = resolve_identity(&config, path)?;

    Ok(LoadedPersona {
        config,
        identity_text,
        source_path: path.to_path_buf(),
    })
}

/// Resolve identity text from inline or file reference.
fn resolve_identity(
    config: &PersonaConfig,
    toml_path: &Path,
) -> Result<Option<String>, PersonaError> {
    // Inline identity takes priority
    if let Some(ref text) = config.prompt.identity {
        return Ok(Some(text.clone()));
    }

    // Try identity_file (resolved relative to TOML parent dir)
    if let Some(ref file_path) = config.prompt.identity_file {
        let parent = toml_path.parent().unwrap_or(Path::new("."));
        let resolved = parent.join(file_path);

        let content = std::fs::read_to_string(&resolved).map_err(|e| PersonaError::ReadError {
            path: resolved.display().to_string(),
            reason: e.to_string(),
        })?;

        return Ok(Some(content));
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("create temp dir")
    }

    #[test]
    fn test_load_valid_persona() {
        let dir = tmp_dir();
        let path = dir.path().join("designer.toml");
        fs::write(
            &path,
            r#"
[persona]
name = "designer"
description = "UI/UX designer"

[prompt]
identity = "You are a UI/UX designer."

[tools]
allow = ["memory_search", "echo"]
"#,
        )
        .unwrap();

        let persona = load_persona(&path).expect("load");
        assert_eq!(persona.name(), "designer");
        assert_eq!(persona.description(), "UI/UX designer");
        assert_eq!(
            persona.identity_text.as_deref(),
            Some("You are a UI/UX designer.")
        );
        assert_eq!(persona.config.tools.allow, vec!["memory_search", "echo"]);
    }

    #[test]
    fn test_name_mismatch_rejected() {
        let dir = tmp_dir();
        let path = dir.path().join("designer.toml");
        fs::write(&path, "[persona]\nname = \"developer\"\n").unwrap();

        let err = load_persona(&path).unwrap_err();
        assert!(matches!(err, PersonaError::ValidationError { .. }));
    }

    #[test]
    fn test_invalid_name_rejected() {
        let dir = tmp_dir();
        let path = dir.path().join("-bad.toml");
        fs::write(&path, "[persona]\nname = \"-bad\"\n").unwrap();

        let err = load_persona(&path).unwrap_err();
        assert!(matches!(err, PersonaError::ValidationError { .. }));
    }

    #[test]
    fn test_allow_deny_mutual_exclusion() {
        let dir = tmp_dir();
        let path = dir.path().join("conflict.toml");
        fs::write(
            &path,
            r#"
[persona]
name = "conflict"

[tools]
allow = ["echo"]
deny = ["shell"]
"#,
        )
        .unwrap();

        let err = load_persona(&path).unwrap_err();
        assert!(matches!(err, PersonaError::ValidationError { .. }));
    }

    #[test]
    fn test_identity_file_resolution() {
        let dir = tmp_dir();
        let identity_dir = dir.path().join("dev");
        fs::create_dir_all(&identity_dir).unwrap();
        fs::write(identity_dir.join("IDENTITY.md"), "You are a developer.").unwrap();

        let path = dir.path().join("dev-persona.toml");
        fs::write(
            &path,
            r#"
[persona]
name = "dev-persona"

[prompt]
identity_file = "dev/IDENTITY.md"
"#,
        )
        .unwrap();

        let persona = load_persona(&path).expect("load");
        assert_eq!(
            persona.identity_text.as_deref(),
            Some("You are a developer.")
        );
    }

    #[test]
    fn test_file_too_large_rejected() {
        let dir = tmp_dir();
        let path = dir.path().join("big.toml");
        let content = format!(
            "[persona]\nname = \"big\"\n\n[prompt]\nidentity = \"{}\"\n",
            "x".repeat(65 * 1024)
        );
        fs::write(&path, content).unwrap();

        let err = load_persona(&path).unwrap_err();
        assert!(matches!(err, PersonaError::ValidationError { .. }));
    }

    #[tokio::test]
    async fn test_discover_from_dir_empty() {
        let dir = tmp_dir();
        let personas = discover_from_dir(dir.path()).await;
        assert!(personas.is_empty());
    }

    #[tokio::test]
    async fn test_discover_from_dir_mixed() {
        let dir = tmp_dir();

        // Valid persona
        fs::write(dir.path().join("good.toml"), "[persona]\nname = \"good\"\n").unwrap();

        // Invalid (name mismatch)
        fs::write(dir.path().join("bad.toml"), "[persona]\nname = \"wrong\"\n").unwrap();

        // Non-TOML file (should be skipped)
        fs::write(dir.path().join("readme.md"), "# Personas").unwrap();

        let personas = discover_from_dir(dir.path()).await;
        assert_eq!(personas.len(), 1);
        assert_eq!(personas[0].name(), "good");
    }
}
