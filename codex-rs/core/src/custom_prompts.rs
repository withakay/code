use codex_protocol::custom_prompts::CustomPrompt;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use tokio::fs;

/// Return the default prompts directory: `$CODEX_HOME/prompts`.
/// If `CODEX_HOME` cannot be resolved, returns `None`.
pub fn default_prompts_dir() -> Option<PathBuf> {
    crate::config::find_codex_home()
        .ok()
        .map(|home| home.join("prompts"))
}

/// Discover prompt files in the given directory, returning entries sorted by name.
/// Non-files are ignored. If the directory does not exist or cannot be read, returns empty.
pub async fn discover_prompts_in(dir: &Path) -> Vec<CustomPrompt> {
    discover_prompts_in_excluding(dir, &HashSet::new()).await
}

/// Discover prompt files in the given directory, excluding any with names in `exclude`.
/// Returns entries sorted by name. Non-files are ignored. Missing/unreadable dir yields empty.
pub async fn discover_prompts_in_excluding(
    dir: &Path,
    exclude: &HashSet<String>,
) -> Vec<CustomPrompt> {
    let mut out: Vec<CustomPrompt> = Vec::new();
    let mut entries = match fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(_) => return out,
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let is_file = entry
            .file_type()
            .await
            .map(|ft| ft.is_file())
            .unwrap_or(false);
        if !is_file {
            continue;
        }
        // Only include Markdown files with a .md extension.
        let is_md = path
            .extension()
            .and_then(|s| s.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("md"))
            .unwrap_or(false);
        if !is_md {
            continue;
        }
        let Some(name) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
        else {
            continue;
        };
        if exclude.contains(&name) {
            continue;
        }
        let content = match fs::read_to_string(&path).await {
            Ok(s) => s,
            Err(_) => continue,
        };
        out.push(CustomPrompt {
            name,
            path,
            content,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Parse a slash-style invocation like "/name args..." and, if it matches a
/// known `CustomPrompt`, return that prompt's content with all instances of
/// `$ARGUMENTS` replaced by the provided arguments. Returns `None` if the text
/// is not a slash invocation or if no matching prompt exists.
pub fn expand_prompt_invocation(text: &str, prompts: &[CustomPrompt]) -> Option<String> {
    // Ignore any leading spaces before the slash.
    let trimmed = text.trim_start_matches(' ');
    let rest = trimmed.strip_prefix('/')?;

    // Identify the command token (up to the first ASCII whitespace).
    let mut name_len = 0usize;
    for &byte in rest.as_bytes() {
        if byte.is_ascii_whitespace() {
            break;
        }
        name_len += 1;
    }

    let name = &rest[..name_len];
    let args = if name_len >= rest.len() {
        ""
    } else {
        let mut s = &rest[name_len..];
        if let Some(first) = s.as_bytes().first() {
            if first.is_ascii_whitespace() {
                s = &s[1..];
            }
        }
        s
    };

    let prompt = prompts.iter().find(|p| p.name == name)?;
    if prompt.content.contains("$ARGUMENTS") {
        Some(prompt.content.replace("$ARGUMENTS", args))
    } else {
        Some(prompt.content.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn empty_when_dir_missing() {
        let tmp = tempdir().expect("create TempDir");
        let missing = tmp.path().join("nope");
        let found = discover_prompts_in(&missing).await;
        assert!(found.is_empty());
    }

    #[tokio::test]
    async fn discovers_and_sorts_files() {
        let tmp = tempdir().expect("create TempDir");
        let dir = tmp.path();
        fs::write(dir.join("b.md"), b"b").unwrap();
        fs::write(dir.join("a.md"), b"a").unwrap();
        fs::create_dir(dir.join("subdir")).unwrap();
        let found = discover_prompts_in(dir).await;
        let names: Vec<String> = found.into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[tokio::test]
    async fn excludes_builtins() {
        let tmp = tempdir().expect("create TempDir");
        let dir = tmp.path();
        fs::write(dir.join("init.md"), b"ignored").unwrap();
        fs::write(dir.join("foo.md"), b"ok").unwrap();
        let mut exclude = HashSet::new();
        exclude.insert("init".to_string());
        let found = discover_prompts_in_excluding(dir, &exclude).await;
        let names: Vec<String> = found.into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["foo"]);
    }

    #[tokio::test]
    async fn skips_non_utf8_files() {
        let tmp = tempdir().expect("create TempDir");
        let dir = tmp.path();
        // Valid UTF-8 file
        fs::write(dir.join("good.md"), b"hello").unwrap();
        // Invalid UTF-8 content in .md file (e.g., lone 0xFF byte)
        fs::write(dir.join("bad.md"), vec![0xFF, 0xFE, b'\n']).unwrap();
        let found = discover_prompts_in(dir).await;
        let names: Vec<String> = found.into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["good"]);
    }

    fn prompt(name: &str, content: &str) -> CustomPrompt {
        CustomPrompt {
            name: name.to_string(),
            path: PathBuf::from(format!("/tmp/{name}.md")),
            content: content.to_string(),
        }
    }

    #[test]
    fn expand_prompt_invocation_replaces_arguments() {
        let prompts = vec![prompt("hello", "Hi $ARGUMENTS!")];
        let expanded = expand_prompt_invocation("/hello world", &prompts)
            .expect("should match prompt");
        assert_eq!(expanded, "Hi world!");
    }

    #[test]
    fn expand_prompt_invocation_handles_multiple_occurrences() {
        let prompts = vec![prompt("echo", "A:$ARGUMENTS B:$ARGUMENTS")];
        let expanded = expand_prompt_invocation("/echo foo bar", &prompts)
            .expect("should match prompt");
        assert_eq!(expanded, "A:foo bar B:foo bar");
    }

    #[test]
    fn expand_prompt_invocation_allows_empty_arguments() {
        let prompts = vec![prompt("hello", "<$ARGUMENTS>")];
        let expanded = expand_prompt_invocation("/hello", &prompts).expect("should match");
        assert_eq!(expanded, "<>");
    }

    #[test]
    fn expand_prompt_invocation_returns_prompt_without_placeholder() {
        let prompts = vec![prompt("hello", "Hi there!")];
        let expanded = expand_prompt_invocation("/hello world", &prompts)
            .expect("should match prompt");
        assert_eq!(expanded, "Hi there!");
    }

    #[test]
    fn expand_prompt_invocation_unknown_prompt_returns_none() {
        let prompts = vec![prompt("hello", "Hi $ARGUMENTS!")];
        assert!(expand_prompt_invocation("/goodbye world", &prompts).is_none());
    }

    #[test]
    fn expand_prompt_invocation_ignores_non_slash_text() {
        let prompts = vec![prompt("hello", "Hi $ARGUMENTS!")];
        assert!(expand_prompt_invocation("hello world", &prompts).is_none());
    }
}
