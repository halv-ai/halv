//! Connects the desktop `agent_status` module to the remote gateway.
//!
//! The module keys each status by the terminal session id, like the panes of the
//! remote protocol. This bridge reads its snapshot for `Pane.status` and turns its
//! change stream into the "panes changed" signal of the gateway backend.

use halv_remote::rpc::{AgentState, AgentStatus, Prompt};
use tauri::{AppHandle, Manager};
use tokio::sync::broadcast;

use super::backend::AgentStatusSource;
use crate::agent_status::{self, AgentStatusEntry, AgentStatusStore, PromptDetail};

/// The agent status source of the running app.
pub(crate) struct DesktopAgentStatus {
    app: AppHandle,
}

impl DesktopAgentStatus {
    pub(crate) fn new(app: AppHandle) -> DesktopAgentStatus {
        DesktopAgentStatus { app }
    }
}

impl AgentStatusSource for DesktopAgentStatus {
    fn available(&self) -> bool {
        self.app.try_state::<AgentStatusStore>().is_some()
    }

    fn status(&self, session_id: &str) -> Option<AgentStatus> {
        agent_status::snapshot(&self.app)
            .iter()
            .find(|entry| entry.session_id == session_id)
            .map(remote_status)
    }

    fn subscribe(&self) -> Option<broadcast::Receiver<()>> {
        let mut entries = agent_status::subscribe(&self.app)?;
        let (changed, receiver) = broadcast::channel(16);
        tauri::async_runtime::spawn(async move {
            loop {
                match entries.recv().await {
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {
                        // The send fails only when no receiver is left.
                        if changed.send(()).is_err() {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        });
        Some(receiver)
    }
}

/// Converts a desktop status entry to the `AgentStatus` of the remote protocol.
pub(crate) fn remote_status(entry: &AgentStatusEntry) -> AgentStatus {
    AgentStatus {
        state: AgentState::from(state_text(entry.status)),
        reason: entry.reason.clone(),
        updated_at: entry.updated_at,
        unread: entry.unread,
        prompt: entry.prompt.as_ref().map(remote_prompt),
    }
}

fn state_text(status: agent_status::AgentStatus) -> &'static str {
    use agent_status::AgentStatus as Desktop;
    match status {
        Desktop::Working => "working",
        Desktop::Done => "done",
        Desktop::NeedsInput => "needsInput",
        Desktop::NeedsApproval => "needsApproval",
        Desktop::Idle => "idle",
        Desktop::Exited => "exited",
        Desktop::Failed => "failed",
        Desktop::Interrupted => "interrupted",
    }
}

fn remote_prompt(prompt: &PromptDetail) -> Prompt {
    match prompt {
        PromptDetail::Approval {
            tool_name,
            command,
            file_path,
            message,
        } => Prompt::Approval {
            tool_name: tool_name.clone(),
            command: command.clone(),
            file_path: file_path.clone(),
            message: message.clone(),
        },
        PromptDetail::Question { text, options } => Prompt::Question {
            text: text.clone(),
            options: options.clone(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_status::StatusSource;

    fn entry(status: agent_status::AgentStatus, prompt: Option<PromptDetail>) -> AgentStatusEntry {
        AgentStatusEntry {
            session_id: "pane-1".to_string(),
            status,
            reason: Some("hook".to_string()),
            updated_at: 1_760_460_000_000,
            unread: true,
            wait_seq: 2,
            turn_seq: 3,
            source: StatusSource::Hook,
            prompt,
        }
    }

    #[test]
    fn every_desktop_state_maps_to_the_contract_text() {
        use agent_status::AgentStatus as Desktop;
        let cases = [
            (Desktop::Working, "working"),
            (Desktop::Done, "done"),
            (Desktop::NeedsInput, "needsInput"),
            (Desktop::NeedsApproval, "needsApproval"),
            (Desktop::Idle, "idle"),
            (Desktop::Exited, "exited"),
            (Desktop::Failed, "failed"),
            (Desktop::Interrupted, "interrupted"),
        ];
        for (desktop, text) in cases {
            let status = remote_status(&entry(desktop, None));
            assert_eq!(status.state, AgentState::from(text));
            assert!(
                !matches!(status.state, AgentState::Other(_)),
                "{text} is a known state"
            );
        }
    }

    #[test]
    fn an_approval_prompt_keeps_its_detail() {
        let status = remote_status(&entry(
            agent_status::AgentStatus::NeedsApproval,
            Some(PromptDetail::Approval {
                tool_name: "Bash".to_string(),
                command: Some("git commit -am wip".to_string()),
                file_path: None,
                message: None,
            }),
        ));
        assert_eq!(status.reason.as_deref(), Some("hook"));
        assert_eq!(status.updated_at, 1_760_460_000_000);
        assert!(status.unread);
        assert_eq!(
            status.prompt,
            Some(Prompt::Approval {
                tool_name: "Bash".to_string(),
                command: Some("git commit -am wip".to_string()),
                file_path: None,
                message: None,
            })
        );
    }

    #[test]
    fn a_question_prompt_keeps_its_options() {
        let status = remote_status(&entry(
            agent_status::AgentStatus::NeedsInput,
            Some(PromptDetail::Question {
                text: "Which branch?".to_string(),
                options: Some(vec!["main".to_string(), "dev".to_string()]),
            }),
        ));
        assert_eq!(
            status.prompt,
            Some(Prompt::Question {
                text: "Which branch?".to_string(),
                options: Some(vec!["main".to_string(), "dev".to_string()]),
            })
        );
    }
}
