use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The baseline protocol. Every hello frame sends it in `proto`, and it never
/// changes. Protocol 1 never loses or changes a request or a response, so
/// every app and supervisor since Halv 0.5 can connect. See ADR 0008.
pub const PROTOCOL_VERSION: u32 = 1;

/// The highest protocol revision that this build supports. The hello frames
/// send it in `max_proto`. A new revision only adds optional fields or new
/// requests, and a peer sends a new request only after the negotiation.
pub const MAX_PROTOCOL_VERSION: u32 = 1;

const _: () = assert!(MAX_PROTOCOL_VERSION >= PROTOCOL_VERSION);

/// Returns the protocol revision that both peers support. A peer that sends
/// no `max_proto` supports only the baseline.
pub fn negotiated_protocol(local_max: u32, peer_max: Option<u32>) -> u32 {
    local_max
        .min(peer_max.unwrap_or(PROTOCOL_VERSION))
        .max(PROTOCOL_VERSION)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenRequest {
    pub id: String,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: String,
    #[serde(default)]
    pub env: Vec<[String; 2]>,
    #[serde(default)]
    pub env_remove: Vec<String>,
    pub cols: u16,
    pub rows: u16,
    #[serde(default)]
    pub meta: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Running,
    Exited,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub pid: u32,
    pub cwd: String,
    pub program: String,
    pub status: SessionStatus,
    pub cols: u16,
    pub rows: u16,
    pub started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exited_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal: Option<String>,
    pub meta: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub session: SessionSummary,
    pub sequence: u64,
    pub scrollback: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Data {
        sequence: u64,
        data: String,
    },
    Exit {
        sequence: u64,
        exit_code: Option<i32>,
        signal: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RequestFrame {
    Hello {
        proto: u32,
        app_version: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_proto: Option<u32>,
    },
    Open {
        id: String,
        program: String,
        #[serde(default)]
        args: Vec<String>,
        cwd: String,
        #[serde(default)]
        env: Vec<[String; 2]>,
        #[serde(default)]
        env_remove: Vec<String>,
        cols: u16,
        rows: u16,
        #[serde(default)]
        meta: Value,
    },
    Write {
        id: String,
        data: String,
    },
    Resize {
        id: String,
        cols: u16,
        rows: u16,
    },
    Close {
        id: String,
    },
    Remove {
        id: String,
    },
    List,
    UpdateMeta {
        id: String,
        meta: Value,
    },
    Shutdown {
        kill_sessions: bool,
    },
    Attach {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from_sequence: Option<u64>,
    },
}

impl RequestFrame {
    /// Returns the lowest protocol revision that accepts this request. A new
    /// request returns the revision that added it, so a client never sends it
    /// to an older supervisor.
    pub fn min_protocol(&self) -> u32 {
        match self {
            Self::Hello { .. }
            | Self::Open { .. }
            | Self::Write { .. }
            | Self::Resize { .. }
            | Self::Close { .. }
            | Self::Remove { .. }
            | Self::List
            | Self::UpdateMeta { .. }
            | Self::Shutdown { .. }
            | Self::Attach { .. } => PROTOCOL_VERSION,
        }
    }
}

impl From<OpenRequest> for RequestFrame {
    fn from(request: OpenRequest) -> Self {
        Self::Open {
            id: request.id,
            program: request.program,
            args: request.args,
            cwd: request.cwd,
            env: request.env,
            env_remove: request.env_remove,
            cols: request.cols,
            rows: request.rows,
            meta: request.meta,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseFrame {
    Hello {
        proto: u32,
        supervisor_version: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        supervisor_fingerprint: Option<String>,
        pid: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_proto: Option<u32>,
    },
    Ok,
    Session {
        session: SessionSummary,
    },
    List {
        sessions: Vec<SessionSummary>,
    },
    Snapshot {
        session: SessionSummary,
        sequence: u64,
        scrollback: String,
    },
    Data {
        sequence: u64,
        data: String,
    },
    Exit {
        sequence: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        signal: Option<String>,
    },
    Error {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    // Halv 0.5.7 to 0.7.1 send these lines. A newer app or supervisor must
    // decode each line and send it back byte for byte. See ADR 0008.
    const PROTOCOL_1_REQUESTS: &[&str] = &[
        r#"{"type":"hello","proto":1,"app_version":"0.7.1"}"#,
        r#"{"type":"open","id":"t1","program":"/bin/zsh","args":["-l"],"cwd":"/tmp","env":[["TERM","xterm-256color"]],"env_remove":["CLAUDECODE"],"cols":120,"rows":32,"meta":{"id":"t1"}}"#,
        r#"{"type":"write","id":"t1","data":"bHMK"}"#,
        r#"{"type":"resize","id":"t1","cols":100,"rows":30}"#,
        r#"{"type":"close","id":"t1"}"#,
        r#"{"type":"remove","id":"t1"}"#,
        r#"{"type":"list"}"#,
        r#"{"type":"update_meta","id":"t1","meta":{"title":"Release checks"}}"#,
        r#"{"type":"shutdown","kill_sessions":false}"#,
        r#"{"type":"attach","id":"t1"}"#,
        r#"{"type":"attach","id":"t1","from_sequence":41}"#,
    ];

    const PROTOCOL_1_RESPONSES: &[&str] = &[
        r#"{"type":"hello","proto":1,"supervisor_version":"0.6.3","supervisor_fingerprint":"0123456789abcdef","pid":1125}"#,
        r#"{"type":"ok"}"#,
        r#"{"type":"session","session":{"id":"t1","pid":4242,"cwd":"/tmp","program":"/bin/zsh","status":"running","cols":120,"rows":32,"started_at":"2026-09-30T02:21:16Z","meta":{"id":"t1"}}}"#,
        r#"{"type":"list","sessions":[{"id":"t1","pid":4242,"cwd":"/tmp","program":"/bin/zsh","status":"exited","cols":120,"rows":32,"started_at":"2026-09-30T02:21:16Z","exited_at":"2026-09-30T03:00:00Z","exit_code":0,"meta":{}}]}"#,
        r#"{"type":"snapshot","session":{"id":"t1","pid":4242,"cwd":"/tmp","program":"/bin/zsh","status":"running","cols":120,"rows":32,"started_at":"2026-09-30T02:21:16Z","meta":null},"sequence":7,"scrollback":"aGk="}"#,
        r#"{"type":"data","sequence":8,"data":"aGk="}"#,
        r#"{"type":"exit","sequence":9,"exit_code":0}"#,
        r#"{"type":"exit","sequence":9,"signal":"SIGHUP"}"#,
        r#"{"type":"error","message":"terminal session `t1` was not found"}"#,
    ];

    #[test]
    fn protocol_1_is_the_permanent_baseline() {
        assert_eq!(
            PROTOCOL_VERSION, 1,
            "protocol 1 is frozen; add a revision through max_proto instead (ADR 0008)"
        );
    }

    #[test]
    fn protocol_1_requests_keep_their_wire_shape() {
        for line in PROTOCOL_1_REQUESTS {
            let frame = serde_json::from_str::<RequestFrame>(line)
                .unwrap_or_else(|error| panic!("failed to decode {line}: {error}"));
            assert_eq!(frame.min_protocol(), PROTOCOL_VERSION, "{line}");
            assert_eq!(serde_json::to_string(&frame).unwrap(), *line);
        }
    }

    #[test]
    fn protocol_1_responses_keep_their_wire_shape() {
        for line in PROTOCOL_1_RESPONSES {
            let frame = serde_json::from_str::<ResponseFrame>(line)
                .unwrap_or_else(|error| panic!("failed to decode {line}: {error}"));
            assert_eq!(serde_json::to_string(&frame).unwrap(), *line);
        }
    }

    #[test]
    fn a_hello_without_max_proto_negotiates_the_baseline() {
        let RequestFrame::Hello { max_proto, .. } =
            serde_json::from_str(PROTOCOL_1_REQUESTS[0]).unwrap()
        else {
            panic!("expected a hello request");
        };
        assert_eq!(max_proto, None);
        assert_eq!(
            negotiated_protocol(MAX_PROTOCOL_VERSION, max_proto),
            PROTOCOL_VERSION
        );

        let ResponseFrame::Hello { max_proto, .. } =
            serde_json::from_str(PROTOCOL_1_RESPONSES[0]).unwrap()
        else {
            panic!("expected a hello response");
        };
        assert_eq!(max_proto, None);
        assert_eq!(
            negotiated_protocol(MAX_PROTOCOL_VERSION, max_proto),
            PROTOCOL_VERSION
        );
    }

    #[test]
    fn protocol_1_peers_ignore_max_proto() {
        // These enums copy the hello frames of Halv 0.5.7 to 0.7.1.
        #[derive(Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        enum Protocol1Request {
            Hello { proto: u32, app_version: String },
        }
        #[derive(Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        enum Protocol1Response {
            Hello {
                proto: u32,
                supervisor_version: String,
                pid: u32,
            },
        }

        let request = serde_json::to_string(&RequestFrame::Hello {
            proto: PROTOCOL_VERSION,
            app_version: "0.7.2".to_string(),
            max_proto: Some(MAX_PROTOCOL_VERSION),
        })
        .unwrap();
        assert!(request.contains(r#""max_proto":"#));
        let Protocol1Request::Hello { proto, app_version } =
            serde_json::from_str(&request).unwrap();
        assert_eq!(proto, 1);
        assert_eq!(app_version, "0.7.2");

        let response = serde_json::to_string(&ResponseFrame::Hello {
            proto: PROTOCOL_VERSION,
            supervisor_version: "0.7.2".to_string(),
            supervisor_fingerprint: Some("0123456789abcdef".to_string()),
            pid: 42,
            max_proto: Some(MAX_PROTOCOL_VERSION),
        })
        .unwrap();
        let Protocol1Response::Hello {
            proto,
            supervisor_version,
            pid,
        } = serde_json::from_str(&response).unwrap();
        assert_eq!(proto, 1);
        assert_eq!(supervisor_version, "0.7.2");
        assert_eq!(pid, 42);
    }

    #[test]
    fn negotiation_picks_the_highest_shared_revision() {
        assert_eq!(negotiated_protocol(1, None), 1);
        assert_eq!(negotiated_protocol(1, Some(1)), 1);
        assert_eq!(negotiated_protocol(1, Some(4)), 1);
        assert_eq!(negotiated_protocol(3, None), 1);
        assert_eq!(negotiated_protocol(3, Some(2)), 2);
        assert_eq!(negotiated_protocol(3, Some(0)), 1);
    }
}
