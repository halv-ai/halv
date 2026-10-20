//! Test helpers (feature `testing`). Use them only in tests.
//!
//! - [`MockRelay`]: an in-process relay with the wire protocol and the checks of section 6.
//! - [`MemoryBackend`], [`MemoryDeviceStore`], and [`TestApprover`]: in-memory host parts.
//!
//! The helpers need a tokio runtime. No helper uses the network outside `127.0.0.1`.

mod memory;
mod relay;

pub use memory::{
    Approval, MemoryBackend, MemoryDeviceStore, TestApprover, DEFAULT_RING_BYTES, MAX_WRITE_BYTES,
};
pub use relay::{Direction, Fault, MockInvite, MockRelay, MockRelayConfig};
