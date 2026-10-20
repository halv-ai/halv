//! The walk that finds git repositories below a folder (ADR 0019).
//!
//! Two scans use the walk. The review scan lists the repositories with
//! changes below the cwd of a terminal. The folder scan finds the
//! repositories of a folder project, with the rules of Orca's nested
//! repository discovery. Both walks go breadth first and read the children
//! of each folder in name order. A walk does not follow a symbolic link and
//! does not look inside a repository that it found.

use serde::Serialize;
use std::{
    cmp::Ordering,
    collections::VecDeque,
    fs,
    ops::ControlFlow,
    path::{Path, PathBuf},
    rc::Rc,
};

/// The deepest level of a repository that the folder scan finds. A child
/// of the folder has level 1.
pub(super) const NESTED_REPO_SCAN_MAX_DEPTH: usize = 3;
/// The repositories that the folder scan returns before it stops.
pub(super) const NESTED_REPO_SCAN_LIMIT: usize = 100;
/// The directory entries that the folder scan reads before it stops.
const NESTED_REPO_SCAN_ENTRY_LIMIT: usize = 50_000;
/// The largest `.gitignore` file that the folder scan reads.
const GITIGNORE_SIZE_LIMIT: u64 = 256 * 1024;

/// Directories that hold dependencies or build output. The folder scan does
/// not look inside them (Orca's list).
const SKIPPED_DIRECTORIES: &[&str] = &[
    "node_modules",
    ".next",
    "dist",
    "build",
    ".cache",
    "vendor",
    "__pycache__",
    ".turbo",
    ".parcel-cache",
];

/// The metadata directories of version control systems.
const VCS_METADATA_DIRECTORIES: &[&str] = &[".git", ".svn", ".hg", ".jj", ".sl", ".repo", "CVS"];

/// The result of a folder scan.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NestedRepoScan {
    /// The canonical folder that the scan started at.
    pub path: String,
    /// True when the folder is a git repository or is inside one. Then the
    /// scan does not look for nested repositories.
    pub git_repo: bool,
    /// The roots of the repositories below the folder, in walk order.
    pub repos: Vec<String>,
    /// True when the scan stopped at a limit before the end of the walk.
    pub truncated: bool,
}

/// The rules of one walk.
pub(super) struct RepoWalkRules {
    /// The deepest level of a repository. A child of the root has level 1.
    pub max_depth: usize,
    /// The directory entries that the walk reads before it stops.
    pub entry_limit: usize,
    /// Returns true for a child directory that the walk must skip. The
    /// arguments are the name and the level of the child.
    pub skip_directory: fn(&str, usize) -> bool,
    /// Applies the `.gitignore` files of the folders that the walk reads.
    pub gitignore: bool,
    /// Accepts a bare repository as a repository.
    pub bare_repositories: bool,
}

/// Tells why a walk ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WalkEnd {
    /// The walk read every folder within the depth.
    Complete,
    /// The walk read the maximum number of directory entries.
    EntryLimit,
    /// The visitor stopped the walk.
    Stopped,
}

struct WalkFolder {
    path: PathBuf,
    depth: usize,
    /// The names from the root to this folder.
    segments: Vec<String>,
    ignore_rules: Rc<Vec<IgnoreRule>>,
}

/// Walks the folders below `root` and calls `visit` for each repository.
/// The walk does not look inside a repository. The root itself is not a
/// candidate.
pub(super) fn walk_repositories(
    root: &Path,
    rules: &RepoWalkRules,
    mut visit: impl FnMut(&Path) -> ControlFlow<()>,
) -> WalkEnd {
    let mut queue = VecDeque::from([WalkFolder {
        path: root.to_path_buf(),
        depth: 0,
        segments: Vec::new(),
        ignore_rules: Rc::new(Vec::new()),
    }]);
    let mut entries_read = 0;

    while let Some(folder) = queue.pop_front() {
        let Ok(entries) = fs::read_dir(&folder.path) else {
            continue;
        };
        let mut children = Vec::new();
        let mut gitignore = None;
        for entry in entries {
            if entries_read >= rules.entry_limit {
                return WalkEnd::EntryLimit;
            }
            entries_read += 1;
            let Ok(entry) = entry else {
                continue;
            };
            // The type of the entry itself: a symbolic link is not a
            // directory here, thus the walk does not follow it.
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            if kind.is_file() && name == ".gitignore" {
                gitignore = Some(entry.path());
            } else if kind.is_dir() && !kind.is_symlink() {
                children.push((name, entry.path()));
            }
        }
        let ignore_rules = match gitignore.filter(|_| rules.gitignore) {
            Some(path) => {
                let added = read_gitignore_rules(&path, folder.segments.len());
                if added.is_empty() {
                    folder.ignore_rules
                } else {
                    let mut combined = folder.ignore_rules.as_ref().clone();
                    combined.extend(added);
                    Rc::new(combined)
                }
            }
            None => folder.ignore_rules,
        };
        children.sort_by(|left, right| compare_names(&left.0, &right.0));

        let child_depth = folder.depth + 1;
        for (name, path) in children {
            if (rules.skip_directory)(&name, child_depth) {
                continue;
            }
            let mut segments = folder.segments.clone();
            segments.push(name);
            if is_ignored(&segments, &ignore_rules) {
                continue;
            }
            if is_repository(&path, rules.bare_repositories) {
                if visit(&path).is_break() {
                    return WalkEnd::Stopped;
                }
                continue;
            }
            if child_depth < rules.max_depth {
                queue.push_back(WalkFolder {
                    path,
                    depth: child_depth,
                    segments,
                    ignore_rules: Rc::clone(&ignore_rules),
                });
            }
        }
    }
    WalkEnd::Complete
}

/// Finds the repositories of a folder project with Orca's rules. `root` must
/// be a canonical directory. When the folder is a repository or is inside
/// one, the scan returns no repositories.
pub(super) fn scan_nested_repos(root: &Path) -> NestedRepoScan {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    scan_nested_repos_below_home(root, home.as_deref())
}

/// The folder scan with an explicit home folder. The home folder and the
/// folders above it return no repositories: a scan there finds the tool
/// repositories in hidden folders, for example `~/.nvm`.
fn scan_nested_repos_below_home(root: &Path, home: Option<&Path>) -> NestedRepoScan {
    let path = root.to_string_lossy().into_owned();
    if is_repository(root, true) || super::git_root_for_path(root).is_some() {
        return NestedRepoScan {
            path,
            git_repo: true,
            repos: Vec::new(),
            truncated: false,
        };
    }
    let home = home.map(|home| home.canonicalize().unwrap_or_else(|_| home.to_path_buf()));
    if home.is_some_and(|home| home.starts_with(root)) {
        return NestedRepoScan {
            path,
            git_repo: false,
            repos: Vec::new(),
            truncated: false,
        };
    }
    let rules = RepoWalkRules {
        max_depth: NESTED_REPO_SCAN_MAX_DEPTH,
        entry_limit: NESTED_REPO_SCAN_ENTRY_LIMIT,
        skip_directory: skip_folder_scan_directory,
        gitignore: true,
        bare_repositories: true,
    };
    let mut repos = Vec::new();
    let end = walk_repositories(root, &rules, |repo| {
        // A linked worktree belongs to its main repository. It is not a
        // project of its own, and the walk does not look inside it.
        if is_linked_worktree(repo) {
            return ControlFlow::Continue(());
        }
        if repos.len() >= NESTED_REPO_SCAN_LIMIT {
            return ControlFlow::Break(());
        }
        repos.push(repo.to_string_lossy().into_owned());
        ControlFlow::Continue(())
    });
    NestedRepoScan {
        path,
        git_repo: false,
        repos,
        truncated: end != WalkEnd::Complete,
    }
}

/// The skip rule of the folder scan: dependency and build directories,
/// version control metadata, and hidden directories below level 1.
fn skip_folder_scan_directory(name: &str, depth: usize) -> bool {
    VCS_METADATA_DIRECTORIES.contains(&name)
        || SKIPPED_DIRECTORIES.contains(&name)
        || (depth > 1 && name.starts_with('.'))
}

/// Reports a repository: a `.git` directory or file, or the `HEAD`,
/// `objects` and `refs` of a bare repository.
pub(super) fn is_repository(directory: &Path, bare: bool) -> bool {
    if fs::metadata(directory.join(".git"))
        .is_ok_and(|metadata| metadata.is_dir() || metadata.is_file())
    {
        return true;
    }
    bare && fs::metadata(directory.join("HEAD")).is_ok_and(|metadata| metadata.is_file())
        && fs::metadata(directory.join("objects")).is_ok_and(|metadata| metadata.is_dir())
        && fs::metadata(directory.join("refs")).is_ok_and(|metadata| metadata.is_dir())
}

/// Reports a linked worktree: a `.git` file that points into the
/// `worktrees` folder of another repository. A submodule points into
/// `modules`, so it is not a linked worktree.
fn is_linked_worktree(directory: &Path) -> bool {
    let marker = directory.join(".git");
    if !fs::metadata(&marker).is_ok_and(|metadata| metadata.is_file() && metadata.len() <= 4096) {
        return false;
    }
    let Ok(content) = fs::read_to_string(&marker) else {
        return false;
    };
    content
        .lines()
        .find_map(|line| line.strip_prefix("gitdir:"))
        .map(|gitdir| Path::new(gitdir.trim()).to_path_buf())
        .and_then(|gitdir| {
            gitdir
                .parent()
                .and_then(Path::file_name)
                .map(|name| name == "worktrees")
        })
        .unwrap_or(false)
}

/// The name order of a folder: case does not count first, then the bytes
/// break a tie, so the order is stable.
fn compare_names(left: &str, right: &str) -> Ordering {
    left.to_lowercase()
        .cmp(&right.to_lowercase())
        .then_with(|| left.cmp(right))
}

/// One line of a `.gitignore` file, as Orca reads it.
#[derive(Debug, Clone)]
struct IgnoreRule {
    segments: Vec<GlobSegment>,
    negate: bool,
    /// A pattern without a slash. It matches a name at each level.
    basename_only: bool,
    /// The level of the folder that holds the `.gitignore` file.
    base_depth: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GlobSegment {
    Literal(String),
    /// A pattern with `*` or `?`.
    Pattern(Vec<char>),
    /// `**` in a path pattern: zero or more names.
    AnyNames,
}

fn read_gitignore_rules(path: &Path, base_depth: usize) -> Vec<IgnoreRule> {
    if !fs::metadata(path).is_ok_and(|metadata| metadata.len() <= GITIGNORE_SIZE_LIMIT) {
        return Vec::new();
    }
    fs::read_to_string(path)
        .map(|content| parse_gitignore_rules(&content, base_depth))
        .unwrap_or_default()
}

fn parse_gitignore_rules(content: &str, base_depth: usize) -> Vec<IgnoreRule> {
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let (negate, line) = match line.strip_prefix('!') {
                Some(rest) => (true, rest),
                None => (false, line),
            };
            let anchored = line.starts_with('/');
            let pattern = line.trim_start_matches('/').trim_end_matches('/');
            if pattern.is_empty() {
                return None;
            }
            let basename_only = !anchored && !pattern.contains('/');
            let segments = if basename_only {
                vec![glob_segment(pattern)]
            } else {
                path_segments(pattern)
            };
            Some(IgnoreRule {
                segments,
                negate,
                basename_only,
                base_depth,
            })
        })
        .collect()
}

fn glob_segment(pattern: &str) -> GlobSegment {
    if pattern.contains('*') || pattern.contains('?') {
        GlobSegment::Pattern(pattern.chars().collect())
    } else {
        GlobSegment::Literal(pattern.to_string())
    }
}

/// Splits a path pattern into segments. A run of `**` becomes one segment,
/// because `**/**` accepts the same paths as `**`.
fn path_segments(pattern: &str) -> Vec<GlobSegment> {
    let mut segments = Vec::new();
    for part in pattern.split('/') {
        if part == "**" {
            if segments.last() != Some(&GlobSegment::AnyNames) {
                segments.push(GlobSegment::AnyNames);
            }
        } else {
            segments.push(glob_segment(part));
        }
    }
    segments
}

/// Applies the rules to a directory. `segments` are the names from the scan
/// root to the directory. The last rule that matches decides.
fn is_ignored(segments: &[String], rules: &[IgnoreRule]) -> bool {
    let mut ignored = false;
    for rule in rules {
        if segments.len() <= rule.base_depth {
            continue;
        }
        let relative = &segments[rule.base_depth..];
        let matches = if rule.basename_only {
            relative
                .iter()
                .any(|segment| segment_matches(&rule.segments[0], segment))
        } else {
            path_matches(&rule.segments, relative)
        };
        if matches {
            ignored = !rule.negate;
        }
    }
    ignored
}

fn segment_matches(segment: &GlobSegment, value: &str) -> bool {
    match segment {
        GlobSegment::Literal(literal) => literal == value,
        GlobSegment::Pattern(pattern) => glob_matches(pattern, value),
        GlobSegment::AnyNames => true,
    }
}

/// Matches `*` and `?` in one name. The match retries only the latest star,
/// so the time stays linear in practice (Orca's matcher).
fn glob_matches(pattern: &[char], value: &str) -> bool {
    let value = value.chars().collect::<Vec<_>>();
    let mut pattern_index = 0;
    let mut value_index = 0;
    let mut star_index = None;
    let mut star_match_index = 0;
    while value_index < value.len() {
        match pattern.get(pattern_index) {
            Some('*') => {
                star_index = Some(pattern_index);
                pattern_index += 1;
                star_match_index = value_index;
            }
            Some(token) if *token == value[value_index] || *token == '?' => {
                pattern_index += 1;
                value_index += 1;
            }
            _ => match star_index {
                Some(star) => {
                    pattern_index = star + 1;
                    star_match_index += 1;
                    value_index = star_match_index;
                }
                None => return false,
            },
        }
    }
    while pattern.get(pattern_index) == Some(&'*') {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

/// Matches a path pattern against all names of `candidate`. The table holds
/// one row per pattern segment, so `**` never makes the match exponential.
fn path_matches(pattern: &[GlobSegment], candidate: &[String]) -> bool {
    // `next[j]`: the pattern after the current segment matches candidate[j..].
    let mut next = vec![false; candidate.len() + 1];
    next[candidate.len()] = true;
    for segment in pattern.iter().rev() {
        let mut current = vec![false; candidate.len() + 1];
        for index in (0..=candidate.len()).rev() {
            current[index] = match segment {
                GlobSegment::AnyNames => {
                    next[index] || (index < candidate.len() && current[index + 1])
                }
                _ => {
                    index < candidate.len()
                        && segment_matches(segment, &candidate[index])
                        && next[index + 1]
                }
            };
        }
        next = current;
    }
    next[0]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::new_uuid_v4;

    struct TestFolder(PathBuf);

    impl TestFolder {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("halv-nested-repos-{name}-{}", new_uuid_v4()));
            fs::create_dir_all(&path).expect("scan root should create");
            Self(path.canonicalize().expect("scan root should canonicalize"))
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn dir(&self, relative: &str) -> PathBuf {
            let path = self.0.join(relative);
            fs::create_dir_all(&path).expect("directory should create");
            path
        }

        /// A repository marker without git: the scan reads only markers.
        fn repo(&self, relative: &str) -> String {
            let path = self.dir(relative);
            fs::create_dir_all(path.join(".git")).expect("marker should create");
            path.to_string_lossy().into_owned()
        }

        fn file(&self, relative: &str, content: &str) {
            let path = self.0.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("parent should create");
            }
            fs::write(path, content).expect("file should write");
        }
    }

    impl Drop for TestFolder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn names(scan: &NestedRepoScan, root: &Path) -> Vec<String> {
        scan.repos
            .iter()
            .map(|repo| {
                Path::new(repo)
                    .strip_prefix(root)
                    .expect("repository should be below the root")
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }

    #[test]
    fn folder_scan_finds_child_repos_breadth_first_in_name_order() {
        let root = TestFolder::new("order");
        root.repo("web");
        root.repo("App");
        root.repo("alpha-folder/z-child");
        root.repo("alpha-folder/m-child");
        root.repo("alpha-folder/nested/grandchild");
        root.repo("gamma-folder/a-child");
        root.dir("empty");

        let scan = scan_nested_repos(root.path());
        assert!(!scan.git_repo);
        assert!(!scan.truncated);
        assert_eq!(scan.path, root.path().to_string_lossy());
        assert_eq!(
            names(&scan, root.path()),
            [
                "App",
                "web",
                "alpha-folder/m-child",
                "alpha-folder/z-child",
                "gamma-folder/a-child",
                "alpha-folder/nested/grandchild",
            ]
        );
    }

    #[test]
    fn folder_scan_stops_at_depth_three() {
        let root = TestFolder::new("depth");
        root.repo("one/two/three");
        root.repo("one/two/deeper/four");

        assert_eq!(
            names(&scan_nested_repos(root.path()), root.path()),
            ["one/two/three"]
        );
    }

    #[test]
    fn folder_scan_skips_dependency_build_vcs_and_hidden_directories() {
        let root = TestFolder::new("skipped");
        // A `.git` directory at the top makes the root a repository, thus
        // the test leaves it out.
        for skipped in SKIPPED_DIRECTORIES
            .iter()
            .chain(VCS_METADATA_DIRECTORIES)
            .filter(|name| **name != ".git")
        {
            root.repo(&format!("{skipped}/repo"));
        }
        root.repo("tools/.hidden/repo");
        // A hidden directory at the top is a candidate, as in Orca.
        root.repo(".dotfiles");
        root.repo("tools/kept");

        assert_eq!(
            names(&scan_nested_repos(root.path()), root.path()),
            [".dotfiles", "tools/kept"]
        );
    }

    #[test]
    fn folder_scan_does_not_look_inside_a_found_repository() {
        let root = TestFolder::new("stop");
        root.repo("service");
        root.repo("service/nested");
        root.repo("service/packages/inner");

        assert_eq!(
            names(&scan_nested_repos(root.path()), root.path()),
            ["service"]
        );
    }

    #[test]
    fn folder_scan_detects_bare_and_worktree_repositories() {
        let root = TestFolder::new("markers");
        root.dir("mirror.git/objects");
        root.dir("mirror.git/refs/nested");
        root.file("mirror.git/HEAD", "ref: refs/heads/main\n");
        fs::create_dir_all(root.path().join("mirror.git/refs/nested/.git"))
            .expect("nested marker should create");
        root.file("module/.git", "gitdir: ../elsewhere/.git/modules/module\n");

        assert_eq!(
            names(&scan_nested_repos(root.path()), root.path()),
            ["mirror.git", "module"]
        );
    }

    #[test]
    fn folder_scan_skips_linked_worktrees_and_their_contents() {
        let root = TestFolder::new("linked");
        root.repo("app");
        root.file(
            "app-feature/.git",
            "gitdir: /elsewhere/app/.git/worktrees/app-feature\n",
        );
        root.repo("app-feature/vendored");
        root.file(
            "tmp/wt-pro/.git",
            "gitdir: ../../app/.git/worktrees/wt-pro\n",
        );
        root.repo("web");

        assert_eq!(
            names(&scan_nested_repos(root.path()), root.path()),
            ["app", "web"]
        );
    }

    #[test]
    fn folder_scan_returns_nothing_for_the_home_folder_and_above() {
        let root = TestFolder::new("home");
        let home = root.dir("me");
        root.repo("me/.nvm");
        root.repo("me/programming/app");
        root.repo("other");

        for folder in [root.path().to_path_buf(), home.clone()] {
            let scan = scan_nested_repos_below_home(&folder, Some(&home));
            assert!(!scan.git_repo);
            assert!(
                scan.repos.is_empty(),
                "{} should give no repos",
                folder.display()
            );
        }
        let below = scan_nested_repos_below_home(&home.join("programming"), Some(&home));
        assert_eq!(below.repos.len(), 1);
        assert_eq!(
            names(
                &scan_nested_repos_below_home(root.path(), None),
                root.path()
            ),
            ["other", "me/programming/app"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn folder_scan_does_not_follow_symbolic_links() {
        let root = TestFolder::new("links");
        let outside = TestFolder::new("links-outside");
        outside.repo("outside-repo");
        outside.repo("group/inner");
        std::os::unix::fs::symlink(outside.path(), root.path().join("linked"))
            .expect("folder link should create");
        std::os::unix::fs::symlink(
            outside.path().join("outside-repo"),
            root.path().join("linked-repo"),
        )
        .expect("repository link should create");
        root.repo("real");

        assert_eq!(
            names(&scan_nested_repos(root.path()), root.path()),
            ["real"]
        );
    }

    #[test]
    fn folder_scan_stops_at_the_repository_limit() {
        let root = TestFolder::new("limit");
        for index in 0..=NESTED_REPO_SCAN_LIMIT {
            root.repo(&format!("repo-{index:03}"));
        }

        let scan = scan_nested_repos(root.path());
        assert_eq!(scan.repos.len(), NESTED_REPO_SCAN_LIMIT);
        assert!(scan.truncated);
        assert_eq!(
            names(&scan, root.path()).last().map(String::as_str),
            Some("repo-099")
        );

        let small = TestFolder::new("limit-exact");
        small.repo("one");
        assert!(!scan_nested_repos(small.path()).truncated);
    }

    #[test]
    fn folder_scan_returns_nothing_for_a_repository() {
        let root = TestFolder::new("selected-repo");
        fs::create_dir_all(root.path().join(".git")).expect("marker should create");
        root.repo("child");

        let scan = scan_nested_repos(root.path());
        assert!(scan.git_repo);
        assert!(scan.repos.is_empty());
    }

    #[test]
    fn folder_scan_returns_nothing_inside_a_repository() {
        let root = TestFolder::new("inside-repo");
        let output = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root.path())
            .output()
            .expect("git should run");
        assert!(output.status.success());
        let folder = root.dir("packages");
        root.repo("packages/vendored");

        let scan = scan_nested_repos(&folder);
        assert!(scan.git_repo);
        assert!(scan.repos.is_empty());
    }

    #[test]
    fn folder_scan_applies_gitignore_files() {
        let root = TestFolder::new("gitignore");
        root.file(
            ".gitignore",
            "# comment\nignored/\n/anchored\nout-*\n!out-kept\n",
        );
        root.repo("ignored/repo");
        root.repo("anchored/repo");
        root.repo("active/anchored/repo");
        root.repo("out-build/repo");
        root.repo("out-kept/repo");
        root.file("active/.gitignore", "local\n");
        root.repo("active/local");
        root.repo("other/local");

        assert_eq!(
            names(&scan_nested_repos(root.path()), root.path()),
            ["other/local", "out-kept/repo", "active/anchored/repo"]
        );
    }

    #[test]
    fn gitignore_path_patterns_match_double_stars() {
        let rules = parse_gitignore_rules("a/**/b\n**/deep\nx/*/y\n", 0);
        let path = |value: &str| value.split('/').map(str::to_string).collect::<Vec<_>>();
        assert!(is_ignored(&path("a/b"), &rules));
        assert!(is_ignored(&path("a/one/two/b"), &rules));
        assert!(!is_ignored(&path("a/one/c"), &rules));
        assert!(is_ignored(&path("deep"), &rules));
        assert!(is_ignored(&path("one/deep"), &rules));
        assert!(is_ignored(&path("x/any/y"), &rules));
        assert!(!is_ignored(&path("x/y"), &rules));

        // A rule of a nested folder applies only below that folder.
        let nested = parse_gitignore_rules("/build-output\n", 1);
        assert!(is_ignored(&path("pkg/build-output"), &nested));
        assert!(!is_ignored(&path("build-output"), &nested));
        assert!(!is_ignored(&path("pkg/sub/build-output"), &nested));

        // Many stars stay fast.
        let stars = parse_gitignore_rules(&format!("{}/x\n", ["**"; 40].join("/a/")), 0);
        assert!(!is_ignored(&path(&["a"; 30].join("/")), &stars));
    }

    #[test]
    fn glob_segments_match_stars_and_question_marks() {
        let pattern = |value: &str| value.chars().collect::<Vec<_>>();
        assert!(glob_matches(&pattern("*.cache"), "app.cache"));
        assert!(glob_matches(&pattern("ab?d"), "abcd"));
        assert!(!glob_matches(&pattern("ab?d"), "abd"));
        assert!(glob_matches(&pattern("a*b*c"), "a-b-b-c"));
        assert!(!glob_matches(&pattern("a*b*c"), "a-b-b-d"));
        assert!(glob_matches(&pattern("**"), "anything"));
    }

    #[test]
    fn walk_reports_the_entry_limit() {
        let root = TestFolder::new("entry-limit");
        for index in 0..5 {
            root.dir(&format!("dir-{index}"));
        }
        root.repo("z-repo");
        let rules = RepoWalkRules {
            max_depth: NESTED_REPO_SCAN_MAX_DEPTH,
            entry_limit: 3,
            skip_directory: skip_folder_scan_directory,
            gitignore: true,
            bare_repositories: true,
        };
        let mut found = 0;
        let end = walk_repositories(root.path(), &rules, |_| {
            found += 1;
            ControlFlow::Continue(())
        });
        assert_eq!(end, WalkEnd::EntryLimit);
        assert_eq!(found, 0);
    }
}
