use serde::Serialize;
use std::{
    collections::HashMap,
    env,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

const MAX_PROMPT_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptScope {
    User,
    Codex,
    Project,
}

impl PromptScope {
    fn precedence(self) -> u8 {
        match self {
            Self::Codex => 0,
            Self::User => 1,
            Self::Project => 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomPrompt {
    pub name: String,
    pub body: String,
    pub scope: PromptScope,
}

struct PromptCandidate {
    prompt: CustomPrompt,
    order: usize,
}

#[tauri::command]
pub fn list_custom_prompts(project_root: Option<String>) -> Vec<CustomPrompt> {
    let home = env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    list_custom_prompts_from_roots(home.as_deref(), project_root.as_deref().map(Path::new))
}

fn list_custom_prompts_from_roots(
    home: Option<&Path>,
    project_root: Option<&Path>,
) -> Vec<CustomPrompt> {
    let mut prompts = HashMap::<String, PromptCandidate>::new();
    let mut next_order = 0;

    if let Some(home) = home {
        // Conflict precedence is project > user (~/.halv) > Codex, even though
        // the user directory is scanned before the Codex-compatible directory.
        scan_prompt_directory(
            &home.join(".halv/prompts"),
            PromptScope::User,
            &mut prompts,
            &mut next_order,
        );
        scan_prompt_directory(
            &home.join(".codex/prompts"),
            PromptScope::Codex,
            &mut prompts,
            &mut next_order,
        );
    }
    if let Some(project_root) = project_root {
        scan_prompt_directory(
            &project_root.join(".halv/prompts"),
            PromptScope::Project,
            &mut prompts,
            &mut next_order,
        );
    }

    let mut prompts = prompts.into_values().collect::<Vec<_>>();
    prompts.sort_by_key(|candidate| candidate.order);
    prompts
        .into_iter()
        .map(|candidate| candidate.prompt)
        .collect()
}

fn scan_prompt_directory(
    directory: &Path,
    scope: PromptScope,
    prompts: &mut HashMap<String, PromptCandidate>,
    next_order: &mut usize,
) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    paths.sort();

    for path in paths {
        let Some(prompt) = read_prompt_file(&path, scope) else {
            continue;
        };
        let key = prompt.name.to_ascii_lowercase();
        let should_replace = match prompts.get(&key) {
            Some(existing) => scope.precedence() > existing.prompt.scope.precedence(),
            None => true,
        };
        let order = *next_order;
        *next_order += 1;
        if should_replace {
            prompts.insert(key, PromptCandidate { prompt, order });
        }
    }
}

fn read_prompt_file(path: &Path, scope: PromptScope) -> Option<CustomPrompt> {
    if path.extension().and_then(|extension| extension.to_str()) != Some("md") {
        return None;
    }
    let name = path.file_stem()?.to_str()?;
    if !valid_prompt_name(name) {
        return None;
    }

    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_PROMPT_BYTES {
        return None;
    }

    let file = File::open(path).ok()?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_PROMPT_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_PROMPT_BYTES {
        return None;
    }
    let body = String::from_utf8(bytes).ok()?;

    Some(CustomPrompt {
        name: name.to_string(),
        body,
        scope,
    })
}

fn valid_prompt_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(name: &str) -> Self {
            let path =
                env::temp_dir().join(format!("halv-prompts-{name}-{}", rand::random::<u64>()));
            fs::create_dir_all(&path).expect("create prompt test directory");
            Self(path)
        }

        fn write(&self, relative_path: &str, contents: impl AsRef<[u8]>) {
            let path = self.0.join(relative_path);
            fs::create_dir_all(path.parent().expect("prompt parent"))
                .expect("create prompt parent");
            fs::write(path, contents).expect("write prompt fixture");
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn scans_in_scope_order_and_applies_case_insensitive_precedence() {
        let directory = TestDirectory::new("precedence");
        let home = directory.0.join("home");
        let project = directory.0.join("project");
        directory.write("home/.halv/prompts/z-user.md", "user");
        directory.write("home/.halv/prompts/Shared.md", "user shared");
        directory.write("home/.codex/prompts/a-codex.md", "codex");
        directory.write("home/.codex/prompts/shared.md", "codex shared");
        directory.write("project/.halv/prompts/m-project.md", "project");
        directory.write("project/.halv/prompts/SHARED.md", "project shared");

        let prompts = list_custom_prompts_from_roots(Some(&home), Some(&project));

        assert_eq!(
            prompts
                .iter()
                .filter(|prompt| !prompt.name.eq_ignore_ascii_case("shared"))
                .map(|prompt| (prompt.name.as_str(), prompt.scope))
                .collect::<Vec<_>>(),
            vec![
                ("z-user", PromptScope::User),
                ("a-codex", PromptScope::Codex),
                ("m-project", PromptScope::Project),
            ]
        );
        let shared = prompts
            .iter()
            .find(|prompt| prompt.name.eq_ignore_ascii_case("shared"))
            .expect("shared prompt");
        assert_eq!(shared.name, "SHARED");
        assert_eq!(shared.body, "project shared");
        assert_eq!(shared.scope, PromptScope::Project);
    }

    #[test]
    fn user_prompts_take_precedence_over_codex_prompts() {
        let directory = TestDirectory::new("user-precedence");
        let home = directory.0.join("home");
        directory.write("home/.halv/prompts/review.md", "user review");
        directory.write("home/.codex/prompts/REVIEW.md", "codex review");

        let prompts = list_custom_prompts_from_roots(Some(&home), None);

        assert_eq!(
            prompts,
            vec![CustomPrompt {
                name: "review".to_string(),
                body: "user review".to_string(),
                scope: PromptScope::User,
            }]
        );
    }

    #[test]
    fn filters_extensions_and_invalid_names_but_accepts_supported_case() {
        let directory = TestDirectory::new("names");
        let home = directory.0.join("home");
        directory.write("home/.halv/prompts/valid-name_2.md", "valid");
        directory.write("home/.halv/prompts/UPPER.md", "upper");
        directory.write("home/.halv/prompts/bad name.md", "space");
        directory.write("home/.halv/prompts/bad.name.md", "dot");
        directory.write("home/.halv/prompts/not-markdown.txt", "text");
        directory.write("home/.halv/prompts/wrong-case.MD", "markdown");

        let prompts = list_custom_prompts_from_roots(Some(&home), None);

        assert_eq!(
            prompts
                .iter()
                .map(|prompt| prompt.name.as_str())
                .collect::<Vec<_>>(),
            vec!["UPPER", "valid-name_2"]
        );
    }

    #[test]
    fn enforces_the_64_kib_size_cap() {
        let directory = TestDirectory::new("size");
        let home = directory.0.join("home");
        directory.write(
            "home/.halv/prompts/at-limit.md",
            vec![b'a'; MAX_PROMPT_BYTES as usize],
        );
        directory.write(
            "home/.halv/prompts/over-limit.md",
            vec![b'b'; MAX_PROMPT_BYTES as usize + 1],
        );

        let prompts = list_custom_prompts_from_roots(Some(&home), None);

        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].name, "at-limit");
        assert_eq!(prompts[0].body.len(), MAX_PROMPT_BYTES as usize);
    }
}
