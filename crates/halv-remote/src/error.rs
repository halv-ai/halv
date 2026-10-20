//! The error type of the crate.
//!
//! Error messages never contain input data, keys, tokens, or plaintext.
//! A caller can log them.

use std::fmt;

use crate::relay_wire::FrameError;

/// The result type of the crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// An error from the protocol code.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A key, an id, or an encoded value has a wrong format.
    /// The text names the value, for example `"host id"`.
    #[error("invalid {0}")]
    Invalid(&'static str),

    /// A pairing offer breaks a rule of section 5.1.
    #[error("invalid pairing offer: {0}")]
    InvalidOffer(OfferError),

    /// A pairing offer has a version above 1.
    /// The device must ask the user to update the app.
    #[error("the pairing offer needs a newer app (offer version {version})")]
    UpdateRequired {
        /// The version in the offer.
        version: u64,
    },

    /// A pairing offer expired more than 60 seconds ago.
    #[error("the pairing offer expired")]
    OfferExpired,

    /// A Noise handshake failed.
    #[error("Noise handshake failed: {0}")]
    Handshake(HandshakeFailure),

    /// A transport message did not decrypt.
    /// The cause can be a changed, replayed, reordered, or truncated message.
    #[error("a transport message did not decrypt")]
    Decrypt,

    /// A transport message broke the framing rules of section 7.6.
    #[error("transport framing error: {0}")]
    Framing(&'static str),

    /// An application message is larger than 8 MiB.
    #[error("the application message is larger than 8 MiB")]
    TooLarge,

    /// An earlier error closed the transport.
    /// The peers must make a new handshake on a new connection.
    #[error("the transport is closed after an earlier error")]
    Poisoned,

    /// A relay binary frame is not valid (section 6.4).
    #[error("invalid relay frame: {0}")]
    Frame(FrameError),

    /// A CBOR value did not encode or decode.
    #[error("CBOR error: {0}")]
    Cbor(&'static str),

    /// The random number generator of the operating system failed.
    #[error("the system random number generator failed")]
    Random,

    /// A library failed in a way that the protocol does not expect.
    #[error("internal error: {0}")]
    Internal(&'static str),
}

impl From<FrameError> for Error {
    fn from(error: FrameError) -> Self {
        Error::Frame(error)
    }
}

/// The rule of section 5.1 that a pairing offer breaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OfferError {
    /// The text is not a `halv://pair?p=<b64u>` URI.
    Uri,
    /// The payload is not a JSON object with the correct field types.
    Json,
    /// The field `v` is missing, or it is not an integer of 1 or more.
    Version,
    /// The field `r` is not a valid relay base URL.
    RelayUrl,
    /// The field `h` is not a valid host id.
    HostId,
    /// The field `k` is not the `b64u` text of 32 bytes, or it is a low-order X25519 key.
    HostKey,
    /// The field `s` is not the `b64u` text of 32 bytes.
    Secret,
    /// The field `e` is missing, it is not an integer of 0 or more,
    /// or it is further in the future than an offer can last.
    Expiry,
    /// The field `n` is not text of at most 64 characters.
    Name,
}

impl fmt::Display for OfferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            OfferError::Uri => "not a halv://pair URI",
            OfferError::Json => "the payload is not valid offer JSON",
            OfferError::Version => "the version field is not valid",
            OfferError::RelayUrl => "the relay URL is not valid",
            OfferError::HostId => "the host id is not valid",
            OfferError::HostKey => "the host key is not valid",
            OfferError::Secret => "the pairing secret is not valid",
            OfferError::Expiry => "the expiry time is not valid",
            OfferError::Name => "the host name is not valid",
        };
        f.write_str(text)
    }
}

/// The cause of a failed Noise handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum HandshakeFailure {
    /// A handshake message did not decrypt.
    /// The cause can be a wrong PSK, a wrong static key, a wrong prologue, or a changed message.
    Decrypt,
    /// A handshake message has a wrong length.
    Malformed,
    /// A handshake payload (P1 or P2) is not valid CBOR of section 7.5.
    Payload,
    /// A static key has a low order, so it authenticates nothing.
    /// A key that comes from a secret key never has a low order.
    LowOrderKey,
    /// The caller used the handshake out of order.
    State,
    /// The Noise library failed for another reason.
    Internal,
}

impl fmt::Display for HandshakeFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            HandshakeFailure::Decrypt => "a handshake message did not decrypt",
            HandshakeFailure::Malformed => "a handshake message has a wrong length",
            HandshakeFailure::Payload => "the handshake payload is not valid",
            HandshakeFailure::LowOrderKey => "a static key has a low order",
            HandshakeFailure::State => "the handshake was used out of order",
            HandshakeFailure::Internal => "internal Noise error",
        };
        f.write_str(text)
    }
}

impl From<snow::Error> for HandshakeFailure {
    fn from(error: snow::Error) -> Self {
        match error {
            snow::Error::Decrypt => HandshakeFailure::Decrypt,
            snow::Error::Input | snow::Error::Dh => HandshakeFailure::Malformed,
            snow::Error::State(_) => HandshakeFailure::State,
            _ => HandshakeFailure::Internal,
        }
    }
}

impl From<snow::Error> for Error {
    fn from(error: snow::Error) -> Self {
        Error::Handshake(error.into())
    }
}
