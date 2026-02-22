//! Persona registry for discovering and accessing loaded personas.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::personas::loader::discover_from_dir;
use crate::personas::types::LoadedPersona;

/// Registry of discovered persona configurations.
///
/// Discovers personas from user and workspace directories.
/// Workspace personas override user personas on name collision.
pub struct PersonaRegistry {
    personas: HashMap<String, LoadedPersona>,
    user_dir: PathBuf,
    workspace_dir: Option<PathBuf>,
}

impl PersonaRegistry {
    /// Create a new registry scanning the given user-global directory.
    pub fn new(user_dir: PathBuf) -> Self {
        Self {
            personas: HashMap::new(),
            user_dir,
            workspace_dir: None,
        }
    }

    /// Add a workspace-specific personas directory.
    pub fn with_workspace_dir(mut self, dir: PathBuf) -> Self {
        self.workspace_dir = Some(dir);
        self
    }

    /// Discover all personas from configured directories.
    ///
    /// User directory is loaded first, then workspace directory.
    /// Workspace personas win on name collision.
    pub async fn discover_all(&mut self) -> Vec<String> {
        self.personas.clear();

        // User dir first (lower priority)
        let user_personas = discover_from_dir(&self.user_dir).await;
        for persona in user_personas {
            self.personas.insert(persona.name().to_string(), persona);
        }

        // Workspace dir second (wins on collision)
        if let Some(ref ws_dir) = self.workspace_dir {
            let ws_personas = discover_from_dir(ws_dir).await;
            for persona in ws_personas {
                let name = persona.name().to_string();
                if self.personas.contains_key(&name) {
                    tracing::debug!("Workspace persona '{}' overrides user persona", name);
                }
                self.personas.insert(name, persona);
            }
        }

        self.personas.keys().cloned().collect()
    }

    /// Get a persona by name.
    pub fn get(&self, name: &str) -> Option<&LoadedPersona> {
        self.personas.get(name)
    }

    /// List all available persona names with descriptions.
    pub fn list(&self) -> Vec<(&str, &str)> {
        let mut entries: Vec<_> = self
            .personas
            .values()
            .map(|p| (p.name(), p.description()))
            .collect();
        entries.sort_by_key(|(name, _)| *name);
        entries
    }
}

impl std::fmt::Debug for PersonaRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PersonaRegistry")
            .field("count", &self.personas.len())
            .field("user_dir", &self.user_dir)
            .field("workspace_dir", &self.workspace_dir)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[tokio::test]
    async fn test_registry_discover() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("dev.toml"),
            "[persona]\nname = \"dev\"\ndescription = \"Developer\"\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("ops.toml"),
            "[persona]\nname = \"ops\"\ndescription = \"Operations\"\n",
        )
        .unwrap();

        let mut registry = PersonaRegistry::new(dir.path().to_path_buf());
        let loaded = registry.discover_all().await;
        assert_eq!(loaded.len(), 2);
        assert!(registry.get("dev").is_some());
        assert!(registry.get("ops").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[tokio::test]
    async fn test_registry_workspace_overrides_user() {
        let user_dir = tempfile::tempdir().unwrap();
        let ws_dir = tempfile::tempdir().unwrap();

        fs::write(
            user_dir.path().join("shared.toml"),
            "[persona]\nname = \"shared\"\ndescription = \"User version\"\n",
        )
        .unwrap();
        fs::write(
            ws_dir.path().join("shared.toml"),
            "[persona]\nname = \"shared\"\ndescription = \"Workspace version\"\n",
        )
        .unwrap();

        let mut registry = PersonaRegistry::new(user_dir.path().to_path_buf())
            .with_workspace_dir(ws_dir.path().to_path_buf());
        registry.discover_all().await;

        let persona = registry.get("shared").expect("found");
        assert_eq!(persona.description(), "Workspace version");
    }

    #[tokio::test]
    async fn test_registry_list_sorted() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("zebra.toml"),
            "[persona]\nname = \"zebra\"\ndescription = \"Z\"\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("alpha.toml"),
            "[persona]\nname = \"alpha\"\ndescription = \"A\"\n",
        )
        .unwrap();

        let mut registry = PersonaRegistry::new(dir.path().to_path_buf());
        registry.discover_all().await;

        let list = registry.list();
        assert_eq!(list[0].0, "alpha");
        assert_eq!(list[1].0, "zebra");
    }
}
