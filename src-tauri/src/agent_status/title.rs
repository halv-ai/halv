//! The agent state that a terminal title shows.
//!
//! Agents put their state in the terminal title (OSC 0 or OSC 2). The rules
//! come from Orca (`shared/agent-title-status.ts`). This module keeps only
//! the rules for the agents that Halv runs:
//!
//! - Claude Code (also the GLM and Grok lanes) writes `✳ <topic>` at rest and
//!   a spinner glyph before the topic while it works. Version 2.1.228 and
//!   later use the quarter circles `◐ ◑`, older versions use braille. A
//!   permission prompt shows `✳` too, so the title cannot tell a wait from
//!   a rest.
//! - Codex writes the folder name (or `<thread> | <folder>`) at rest, a
//!   braille spinner before it while it works, and
//!   `[ ! ] Action Required | …` while an approval waits. Codex also shows
//!   the spinner for a moment while it starts, before any prompt.
//! - Gemini CLI puts `✋` (action required), `✦` or `⏲` (working), or `◇`
//!   (ready) in its title.
//! - Other titles count only when they name an agent, for example
//!   `codex ready` or `gemini - action required`.
//!
//! The pane knows its agent, so a spinner glyph alone counts as work.

use super::machine::TitleStatus;
use super::PaneAgent;

const CLAUDE_IDLE: char = '\u{2733}'; // ✳
const GEMINI_WORKING: char = '\u{2726}'; // ✦
const GEMINI_SILENT_WORKING: char = '\u{23f2}'; // ⏲
const GEMINI_IDLE: char = '\u{25c7}'; // ◇
const GEMINI_PERMISSION: char = '\u{270b}'; // ✋

/// Agent names that make a plain title an agent title. Orca keeps a longer
/// list for the agents that it runs. Kimi is not in the list: no capture
/// shows its title, and a static title with its name would hide the webview
/// fallback for good.
const AGENT_NAMES: [&str; 7] = [
    "claude",
    "codex",
    "gemini",
    "antigravity",
    "agy",
    "grok",
    "openclaude",
];
const WINDOWS_SUFFIXES: [&str; 4] = [".exe", ".cmd", ".bat", ".ps1"];
const PERMISSION_WORDS: [&str; 3] = ["action required", "permission", "waiting"];
const IDLE_WORDS: [&str; 3] = ["ready", "idle", "done"];
const WORKING_WORDS: [&str; 3] = ["working", "thinking", "running"];

/// The state in the title of a pane that runs `agent`, or `None` when the
/// title says nothing about the agent.
pub(crate) fn classify_title(agent: PaneAgent, title: &str) -> Option<TitleStatus> {
    let title = title.trim();
    if title.is_empty() || is_claude_management_title(title) {
        return None;
    }
    match agent {
        PaneAgent::ClaudeFamily => claude_title(title).or_else(|| generic_title(title)),
        PaneAgent::Codex => Some(codex_title(title)),
        PaneAgent::Gemini | PaneAgent::Antigravity | PaneAgent::Kimi => generic_title(title),
    }
}

/// Claude writes its state as the first glyph. Orca: "Claude-specific
/// prefixes must win", because the topic can name other agents or glyphs.
fn claude_title(title: &str) -> Option<TitleStatus> {
    let first = title.chars().next()?;
    if first == CLAUDE_IDLE || title.starts_with("* ") {
        Some(TitleStatus::Idle)
    } else if is_spinner(first) || title.starts_with(". ") {
        Some(TitleStatus::Working)
    } else {
        None
    }
}

/// A Codex title is always about Codex: Codex is the only writer of its
/// pane title. A title without a spinner is the rest title. The words of
/// a thread name do not count, because the user picks them.
fn codex_title(title: &str) -> TitleStatus {
    let head = title.split(" | ").next().unwrap_or_default().trim();
    let head = head
        .strip_prefix('[')
        .and_then(|rest| rest.split_once(']'))
        .map_or(head, |(_, rest)| rest.trim());
    if head.eq_ignore_ascii_case("action required") {
        TitleStatus::Permission
    } else if title.chars().any(is_spinner) {
        TitleStatus::Working
    } else {
        TitleStatus::Idle
    }
}

/// Orca's `computeAgentStatusFromTitle`, without the rules of agents that
/// Halv does not run.
fn generic_title(title: &str) -> Option<TitleStatus> {
    if title.contains(GEMINI_PERMISSION) {
        return Some(TitleStatus::Permission);
    }
    if title.contains(GEMINI_WORKING) || title.contains(GEMINI_SILENT_WORKING) {
        return Some(TitleStatus::Working);
    }
    if title.contains(GEMINI_IDLE) {
        return Some(TitleStatus::Idle);
    }
    let mut characters = title.chars();
    if characters.next() == Some(CLAUDE_IDLE) && matches!(characters.next(), None | Some(' ')) {
        return Some(TitleStatus::Idle);
    }
    if title.chars().any(is_spinner) {
        return Some(TitleStatus::Working);
    }
    let lower = title.to_lowercase();
    if !AGENT_NAMES.iter().any(|name| has_agent_name(&lower, name)) {
        return None;
    }
    if PERMISSION_WORDS.iter().any(|word| lower.contains(word)) {
        return Some(TitleStatus::Permission);
    }
    if IDLE_WORDS.iter().any(|word| has_keyword(&lower, word)) {
        return Some(TitleStatus::Idle);
    }
    if WORKING_WORDS.iter().any(|word| has_keyword(&lower, word)) {
        return Some(TitleStatus::Working);
    }
    if title.starts_with(". ") {
        return Some(TitleStatus::Working);
    }
    // A bare agent name, or `* <name>`, is an agent at rest.
    Some(TitleStatus::Idle)
}

/// A braille spinner frame, or a quarter circle of Claude Code 2.1.228 and
/// later. Orca reserves the whole quarter-circle block.
fn is_spinner(character: char) -> bool {
    matches!(character, '\u{2800}'..='\u{28ff}' | '\u{25d0}'..='\u{25d3}')
}

fn is_word(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

/// A character that joins a name to a path or a compound word.
fn joins_token(character: char) -> bool {
    is_word(character) || matches!(character, '.' | '/' | '\\' | '-')
}

/// True when `name` is a whole token in the lowercase title `lower`: not part
/// of a path such as `~/codex/ready`, a compound such as `claude-scratch`, or
/// a longer word. A Windows launcher suffix such as `.exe` can follow it.
fn has_agent_name(lower: &str, name: &str) -> bool {
    find_tokens(lower, name).any(|(before, after)| {
        let after = WINDOWS_SUFFIXES
            .iter()
            .find_map(|suffix| after.strip_prefix(suffix))
            .unwrap_or(after);
        !before.is_some_and(joins_token) && !after.chars().next().is_some_and(joins_token)
    })
}

/// True when `word` is a status word in `lower`. As in Orca, the left side
/// rejects paths and compounds, and the right side allows punctuation such
/// as `done.`.
fn has_keyword(lower: &str, word: &str) -> bool {
    find_tokens(lower, word).any(|(before, after)| {
        !before.is_some_and(joins_token)
            && !after
                .chars()
                .next()
                .is_some_and(|next| is_word(next) || next == '-')
    })
}

/// Each place of `needle` in `haystack`, as the character before it and the
/// text after it.
fn find_tokens<'a>(
    haystack: &'a str,
    needle: &'a str,
) -> impl Iterator<Item = (Option<char>, &'a str)> + 'a {
    haystack.match_indices(needle).map(move |(index, _)| {
        (
            haystack[..index].chars().next_back(),
            &haystack[index + needle.len()..],
        )
    })
}

/// `claude agents` is the agent manager of Claude, not a session.
fn is_claude_management_title(title: &str) -> bool {
    let lower = title.trim().to_lowercase();
    let Some(command) = lower.strip_suffix("agents") else {
        return false;
    };
    if !command.ends_with(char::is_whitespace) {
        return false;
    }
    let command = command.trim();
    let command = ['"', '\'']
        .iter()
        .find_map(|quote| {
            command
                .strip_prefix(*quote)
                .and_then(|rest| rest.strip_suffix(*quote))
        })
        .unwrap_or(command);
    let program = command.rsplit(['/', '\\']).next().unwrap_or(command);
    let program = WINDOWS_SUFFIXES
        .iter()
        .find_map(|suffix| program.strip_suffix(suffix))
        .unwrap_or(program);
    program == "claude"
}

#[cfg(test)]
mod tests {
    use super::*;
    use TitleStatus::{Idle, Permission, Working};

    fn check(agent: PaneAgent, cases: &[(&str, Option<TitleStatus>)]) {
        for (title, expected) in cases {
            assert_eq!(
                classify_title(agent, title),
                *expected,
                "{agent:?}: {title:?}"
            );
        }
    }

    #[test]
    fn claude_titles_from_real_captures() {
        // Orca captured these from Claude Code 2.1.227, 2.1.228, and 2.1.291.
        check(
            PaneAgent::ClaudeFamily,
            &[
                ("\u{2733} Claude Code", Some(Idle)),
                ("\u{25d0} Claude Code", Some(Working)),
                ("\u{25d1} Claude Code", Some(Working)),
                ("\u{25d2} Claude Code", Some(Working)),
                ("\u{25d3} Claude Code", Some(Working)),
                ("\u{25d1} Say hi in one word", Some(Working)),
                ("\u{2733} Say hi in one word", Some(Idle)),
                ("\u{2802} Claude Code", Some(Working)),
                ("\u{2810} Say hi in one word", Some(Working)),
                ("\u{25d0} 500-row markdown table", Some(Working)),
                (
                    "\u{2733} Foreground interrupt verification sleep",
                    Some(Idle),
                ),
                ("\u{2733} User acknowledgment and confirmation", Some(Idle)),
                ("\u{2733}", Some(Idle)),
                (". claude", Some(Working)),
                ("* claude", Some(Idle)),
            ],
        );
    }

    #[test]
    fn the_claude_prefix_wins_over_words_in_the_topic() {
        check(
            PaneAgent::ClaudeFamily,
            &[
                // Orca reads the Gemini glyph first. The pane is Claude, so
                // the Claude prefix decides.
                (
                    "\u{2733} Fix the \u{270b} emoji in gemini output",
                    Some(Idle),
                ),
                ("\u{25d0} Ask codex - permission flow", Some(Working)),
                ("\u{2733} codex working notes", Some(Idle)),
                // Without a Claude prefix, Orca's rules apply.
                ("claude - action required", Some(Permission)),
                ("Claude ready", Some(Idle)),
                ("Claude Code", Some(Idle)),
                ("claude working on task", Some(Working)),
                ("claude.bat working", Some(Working)),
                ("claude agents", None),
                ("  Claude Agents  ", None),
                ("\"/usr/local/bin/claude\" agents", None),
                (
                    "C:\\Users\\dev\\AppData\\Roaming\\npm\\claude.cmd agents",
                    None,
                ),
                ("claude agents working", Some(Working)),
                ("claude-scratch", None),
                ("/usr/local/bin/claude/notes", None),
                ("zsh", None),
                ("", None),
                ("   ", None),
            ],
        );
    }

    #[test]
    fn codex_titles_from_real_captures() {
        // Orca captured these from codex-cli 0.150.1 to 0.162.1. The start
        // animation uses the same spinner as a turn.
        check(
            PaneAgent::Codex,
            &[
                ("workspace-xxxxxxxxxxxxxx", Some(Idle)),
                ("\u{2839} workspace-xxxxxxxxxxxxxx", Some(Working)),
                ("scratch", Some(Idle)),
                ("\u{280b} scratch", Some(Working)),
                ("\u{2819} renaming... \u{2819} | scratch", Some(Working)),
                ("\u{2834} \u{2834} | scratch", Some(Working)),
                ("\u{2834} | detect-at-capcity-codex", Some(Working)),
                ("detect-at-capcity-codex", Some(Idle)),
                (
                    "\u{280b} Plan notes.md addition | codex-plan-scratch",
                    Some(Working),
                ),
                ("Plan notes.md addition | codex-plan-scratch", Some(Idle)),
                (
                    "[ . ] Action Required | \u{280b} | scratch",
                    Some(Permission),
                ),
                (
                    "[ ! ] Action Required | Create approved.txt | scratch",
                    Some(Permission),
                ),
                ("[ . ] Action Required | scratch", Some(Permission)),
                ("sta-8628-bug-worker-s...", Some(Idle)),
                // A thread name that mentions the words is not a wait.
                ("Fix action required banner | scratch", Some(Idle)),
                ("Fix codex permission prompt | halv", Some(Idle)),
                ("codex working on the parser | halv", Some(Idle)),
                ("", None),
            ],
        );
    }

    #[test]
    fn gemini_glyphs() {
        check(
            PaneAgent::Gemini,
            &[
                ("\u{270b} Gemini CLI", Some(Permission)),
                ("\u{2726} Gemini CLI", Some(Working)),
                ("\u{25c7} Gemini CLI", Some(Idle)),
                ("\u{23f2}  Working\u{2026} (my-project)", Some(Working)),
                ("\u{270b}\u{2726} Gemini CLI", Some(Permission)),
                ("gemini waiting for input", Some(Permission)),
                ("gemini thinking", Some(Working)),
                ("gemini", Some(Idle)),
                ("~/gemini-project", None),
            ],
        );
    }

    #[test]
    fn orca_generic_rules_for_other_agents() {
        check(
            PaneAgent::Antigravity,
            &[
                ("antigravity gemini 3 pro", Some(Idle)),
                ("agy - gemini 2 flash", Some(Idle)),
                ("agy-nightly", None),
                ("\u{280b} anything", Some(Working)),
            ],
        );
        check(
            PaneAgent::Kimi,
            &[
                // No capture shows a Kimi title, so its name does not count.
                ("kimi ready", None),
                ("Kimi working.", None),
                ("kimi - action required", None),
                ("grok done", Some(Idle)),
                ("codex.exe", Some(Idle)),
                ("Codex done.", Some(Idle)),
                ("codex idle!", Some(Idle)),
                // Paths, compounds, and longer words are not agent titles.
                ("~/codex/ready", None),
                ("~/codex/working", None),
                ("C:\\tools\\codex\\run", None),
                ("C:\\codex\\working", None),
                ("codex.working", None),
                ("review-14600-codex", None),
                ("timestamp ready", None),
                ("android build running", None),
                ("~/codex-scratch", None),
                ("vim myfile.ts", None),
                ("nwparker@mac: ~/orca", None),
                ("npm run dev", None),
                ("bash", None),
            ],
        );
    }

    #[test]
    fn status_words_reject_paths_but_allow_punctuation() {
        assert!(has_keyword("codex done.", "done"));
        assert!(has_keyword("thinking...", "thinking"));
        assert!(!has_keyword("~/codex/working", "working"));
        assert!(!has_keyword("reworking", "working"));
        assert!(!has_keyword("is-thinking-cap", "thinking"));
        assert!(has_agent_name("codex.exe", "codex"));
        assert!(has_agent_name("tmux | claude - action required", "claude"));
        assert!(!has_agent_name("openclaude", "claude"));
        assert!(!has_agent_name("codex.working", "codex"));
    }
}
