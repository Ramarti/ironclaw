//! Persona-based tool filtering and shell command matching.

use crate::llm::ToolDefinition;
use crate::personas::types::LoadedPersona;

/// Result of filtering tools through a persona's access control.
#[derive(Debug)]
pub struct PersonaFilterResult {
    /// Tools that passed the filter.
    pub tools: Vec<ToolDefinition>,

    /// Names of tools that were removed.
    pub removed_tools: Vec<String>,

    /// Human-readable explanation of what was filtered.
    pub explanation: String,
}

/// Filter tool definitions according to a persona's access control rules.
///
/// - Allowlist mode: keep only tools whose names are in `allow`.
/// - Denylist mode: remove tools whose names are in `deny`.
/// - Neither: pass through unchanged.
/// - If `shell.allowed_commands` is empty and shell not in allow list:
///   remove the `shell` tool.
pub fn filter_tools_for_persona(
    tools: &[ToolDefinition],
    persona: &LoadedPersona,
) -> PersonaFilterResult {
    let access = &persona.config.tools;
    let shell_access = &persona.config.shell;

    if access.is_unrestricted() {
        // No tool restrictions, but check shell access
        let (tools, removed) = maybe_remove_shell(tools, shell_access);
        return PersonaFilterResult {
            explanation: if removed.is_empty() {
                format!("Persona '{}': all tools available", persona.name())
            } else {
                format!(
                    "Persona '{}': shell removed (no allowed commands)",
                    persona.name()
                )
            },
            tools,
            removed_tools: removed,
        };
    }

    if !access.allow.is_empty() {
        // Allowlist mode
        let mut kept = Vec::new();
        let mut removed = Vec::new();

        for tool in tools {
            if access.allow.contains(&tool.name) {
                kept.push(tool.clone());
            } else {
                removed.push(tool.name.clone());
            }
        }

        // Even if shell is in the allow list, remove it when no commands
        // are permitted
        if shell_access.allowed_commands.is_empty()
            && let Some(pos) = kept.iter().position(|t| t.name == "shell")
        {
            removed.push(kept.remove(pos).name);
        }

        return PersonaFilterResult {
            explanation: format!(
                "Persona '{}': allowlist mode, {} tools available, {} removed",
                persona.name(),
                kept.len(),
                removed.len(),
            ),
            tools: kept,
            removed_tools: removed,
        };
    }

    // Denylist mode
    let mut kept = Vec::new();
    let mut removed = Vec::new();

    for tool in tools {
        if access.deny.contains(&tool.name) {
            removed.push(tool.name.clone());
        } else {
            kept.push(tool.clone());
        }
    }

    // Remove shell if no commands are permitted
    if shell_access.allowed_commands.is_empty()
        && let Some(pos) = kept.iter().position(|t| t.name == "shell")
    {
        removed.push(kept.remove(pos).name);
    }

    PersonaFilterResult {
        explanation: format!(
            "Persona '{}': denylist mode, {} tools available, {} removed",
            persona.name(),
            kept.len(),
            removed.len(),
        ),
        tools: kept,
        removed_tools: removed,
    }
}

/// Remove the `shell` tool if no commands are allowed.
fn maybe_remove_shell(
    tools: &[ToolDefinition],
    shell_access: &crate::personas::types::PersonaShellAccess,
) -> (Vec<ToolDefinition>, Vec<String>) {
    if shell_access.allowed_commands.is_empty() {
        let mut kept = Vec::with_capacity(tools.len());
        let mut removed = Vec::new();
        for tool in tools {
            if tool.name == "shell" {
                removed.push(tool.name.clone());
            } else {
                kept.push(tool.clone());
            }
        }
        (kept, removed)
    } else {
        (tools.to_vec(), Vec::new())
    }
}

/// Check if a command matches any of the given glob-style patterns.
///
/// Pattern syntax:
/// - `*` matches any sequence of characters
/// - Patterns are matched against the full command string
/// - Matching is case-insensitive
pub fn matches_command_pattern(cmd: &str, patterns: &[String]) -> bool {
    let cmd_lower = cmd.to_lowercase();

    for pattern in patterns {
        if pattern == "*" {
            return true;
        }

        if glob_match(&cmd_lower, &pattern.to_lowercase()) {
            return true;
        }
    }

    false
}

/// Simple glob matching: `*` matches any sequence of characters.
fn glob_match(text: &str, pattern: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();

    if parts.len() == 1 {
        // No wildcards — exact match
        return text == pattern;
    }

    let mut pos = 0;

    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }

        if i == 0 {
            // First part must match from the start
            if !text.starts_with(part) {
                return false;
            }
            pos = part.len();
        } else if i == parts.len() - 1 {
            // Last part must match at the end
            if !text[pos..].ends_with(part) {
                return false;
            }
            pos = text.len();
        } else {
            // Middle parts can match anywhere after current position
            match text[pos..].find(part) {
                Some(idx) => pos += idx + part.len(),
                None => return false,
            }
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tool(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: String::new(),
            parameters: serde_json::json!({}),
        }
    }

    fn make_persona(allow: Vec<&str>, deny: Vec<&str>, shell_cmds: Vec<&str>) -> LoadedPersona {
        LoadedPersona {
            config: crate::personas::types::PersonaConfig {
                persona: crate::personas::types::PersonaMeta {
                    name: "test".to_string(),
                    description: String::new(),
                },
                prompt: Default::default(),
                tools: crate::personas::types::PersonaToolAccess {
                    allow: allow.into_iter().map(String::from).collect(),
                    deny: deny.into_iter().map(String::from).collect(),
                },
                shell: crate::personas::types::PersonaShellAccess {
                    allowed_commands: shell_cmds.into_iter().map(String::from).collect(),
                },
                sandbox: Default::default(),
            },
            identity_text: None,
            source_path: std::path::PathBuf::new(),
        }
    }

    #[test]
    fn test_allowlist_mode() {
        let tools = vec![make_tool("echo"), make_tool("shell"), make_tool("http")];
        let persona = make_persona(vec!["echo", "http"], vec![], vec!["*"]);

        let result = filter_tools_for_persona(&tools, &persona);
        assert_eq!(result.tools.len(), 2);
        assert!(result.tools.iter().any(|t| t.name == "echo"));
        assert!(result.tools.iter().any(|t| t.name == "http"));
        assert_eq!(result.removed_tools, vec!["shell"]);
    }

    #[test]
    fn test_denylist_mode() {
        let tools = vec![make_tool("echo"), make_tool("shell"), make_tool("http")];
        let persona = make_persona(vec![], vec!["shell"], vec![]);

        let result = filter_tools_for_persona(&tools, &persona);
        // shell is removed by deny, echo and http remain
        assert_eq!(result.tools.len(), 2);
        assert!(result.tools.iter().any(|t| t.name == "echo"));
        assert!(result.tools.iter().any(|t| t.name == "http"));
        assert!(result.removed_tools.contains(&"shell".to_string()));
    }

    #[test]
    fn test_unrestricted_with_no_shell() {
        let tools = vec![make_tool("echo"), make_tool("shell")];
        let persona = make_persona(vec![], vec![], vec![]);

        let result = filter_tools_for_persona(&tools, &persona);
        assert_eq!(result.tools.len(), 1);
        assert_eq!(result.tools[0].name, "echo");
        assert_eq!(result.removed_tools, vec!["shell"]);
    }

    #[test]
    fn test_unrestricted_with_shell_allowed() {
        let tools = vec![make_tool("echo"), make_tool("shell")];
        let persona = make_persona(vec![], vec![], vec!["git *"]);

        let result = filter_tools_for_persona(&tools, &persona);
        assert_eq!(result.tools.len(), 2);
        assert!(result.removed_tools.is_empty());
    }

    #[test]
    fn test_command_pattern_wildcard() {
        assert!(matches_command_pattern("anything", &["*".to_string()]));
    }

    #[test]
    fn test_command_pattern_prefix() {
        let patterns = vec!["git *".to_string()];
        assert!(matches_command_pattern("git status", &patterns));
        assert!(matches_command_pattern("git push origin main", &patterns));
        assert!(!matches_command_pattern("rm -rf /", &patterns));
    }

    #[test]
    fn test_command_pattern_exact() {
        let patterns = vec!["ls".to_string()];
        assert!(matches_command_pattern("ls", &patterns));
        assert!(!matches_command_pattern("ls -la", &patterns));
    }

    #[test]
    fn test_command_pattern_suffix() {
        let patterns = vec!["*.py".to_string()];
        assert!(matches_command_pattern("script.py", &patterns));
        assert!(!matches_command_pattern("script.sh", &patterns));
    }

    #[test]
    fn test_command_pattern_case_insensitive() {
        let patterns = vec!["Git *".to_string()];
        assert!(matches_command_pattern("git status", &patterns));
        assert!(matches_command_pattern("GIT STATUS", &patterns));
    }

    #[test]
    fn test_command_pattern_multiple() {
        let patterns = vec!["git *".to_string(), "npm run *".to_string()];
        assert!(matches_command_pattern("git status", &patterns));
        assert!(matches_command_pattern("npm run test", &patterns));
        assert!(!matches_command_pattern("rm -rf /", &patterns));
    }

    #[test]
    fn test_glob_match_middle_wildcard() {
        assert!(glob_match("cargo test --release", "cargo * --release"));
        assert!(!glob_match("cargo test --debug", "cargo * --release"));
    }

    #[test]
    fn test_allowlist_removes_shell_when_no_commands() {
        let tools = vec![make_tool("echo"), make_tool("shell")];
        // shell is in the allow list, but no commands are allowed
        let persona = make_persona(vec!["echo", "shell"], vec![], vec![]);

        let result = filter_tools_for_persona(&tools, &persona);
        assert_eq!(result.tools.len(), 1);
        assert_eq!(result.tools[0].name, "echo");
    }
}
