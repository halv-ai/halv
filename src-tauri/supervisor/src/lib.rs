pub mod agent_hook;
mod protocol;
mod scrollback;

pub use protocol::{
    negotiated_protocol, Event, OpenRequest, RequestFrame, ResponseFrame, SessionStatus,
    SessionSummary, Snapshot, MAX_PROTOCOL_VERSION, PROTOCOL_VERSION,
};
pub use scrollback::ScrollbackRing;

pub const SUPERVISOR_VERSION: &str = env!("HALV_APP_VERSION");
pub const SUPERVISOR_FINGERPRINT: &str = env!("HALV_SUPERVISOR_FINGERPRINT");

#[cfg(unix)]
mod client;
#[cfg(unix)]
pub use client::{AttachEvents, SupervisorClient};

#[cfg(unix)]
pub mod server;
