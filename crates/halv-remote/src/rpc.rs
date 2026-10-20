//! The application messages (section 8).
//!
//! Every application message is a CBOR map with text keys:
//!
//! ```text
//! request   { "t": "q", "id": u32, "m": text, "p": map }
//! response  { "t": "r", "id": u32, "ok": true,  "v": any }
//!           { "t": "r", "id": u32, "ok": false, "err": { "code": text, "msg": text } }
//! event     { "t": "e", "m": text, "p": map }
//! ```
//!
//! - In version 1, only the device sends requests. Both peers send events.
//! - Every receiver ignores unknown fields.
//! - An unknown method gets the error `unknown_method`. A receiver ignores an unknown event.
//! - A text value that the contract leaves open (`platform`, `kind`, error codes) keeps
//!   an unknown value in an `Other` variant.
//!
//! The host dispatches requests with [`SessionRequest::parse`] and [`PairRequestParams::parse`].
//! The device reads events with [`HostEvent::parse`]. The [`Method`] trait links each method
//! name to its params and result types.
//!
//! `Debug` output of messages and terminal payloads shows no content, so a log line cannot
//! leak a token or a keystroke.

use std::fmt;

use ciborium::Value;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::{Error, Result};
use crate::ids::{DeviceId, RequestId};
use crate::keys::RelayToken;

/// The time that the device waits for a response (15 seconds).
pub const REQUEST_TIMEOUT_MS: u64 = 15_000;

/// The time that the device waits for the response to `pair.request` (130 seconds).
pub const PAIR_REQUEST_TIMEOUT_MS: u64 = 130_000;

/// The time that the host waits for the user to allow a device (120 seconds).
/// No answer means "deny", and the response is the error `timeout`.
pub const APPROVAL_TIMEOUT_MS: u64 = 120_000;

/// The longest device name of `pair.request`, in characters.
pub const MAX_DEVICE_NAME_CHARS: usize = 64;

/// The longest platform of `pair.request`, in characters (section 8.3).
pub const MAX_PLATFORM_CHARS: usize = 32;

/// The longest device model of `pair.request`, in characters (section 8.3).
pub const MAX_MODEL_CHARS: usize = 64;

/// The largest `data` of `terminal.input` (4 KiB).
pub const MAX_TERMINAL_INPUT: usize = 4 * 1024;

/// The largest `text` of `terminal.paste` in bytes of UTF-8 (64 KiB).
pub const MAX_TERMINAL_PASTE: usize = 64 * 1024;

/// The most terminal subscriptions of one session.
pub const MAX_TERMINAL_SUBSCRIPTIONS: usize = 8;

/// The host sends `terminal.data` this time after the first collected byte (section 8.8).
pub const OUTPUT_BATCH_DELAY_MS: u64 = 50;

/// The host sends `terminal.data` when it collected this many bytes.
pub const OUTPUT_BATCH_BYTES: usize = 32 * 1024;

/// The device sends at most one `terminal.ack` in this time.
pub const ACK_INTERVAL_MS: u64 = 250;

/// Above this many unacknowledged bytes, the host stops the data of a subscription.
pub const FLOW_PAUSE_BYTES: usize = 512 * 1024;

/// When an ack brings the unacknowledged bytes below this count,
/// the host sends a new snapshot and continues.
pub const FLOW_RESUME_BYTES: usize = 128 * 1024;

/// The CBOR nesting limit of an application message.
const RECURSION_LIMIT: usize = 32;

/// The method names (sections 8.3 and 8.4).
pub mod method {
    /// `pair.request`: the only method of a pairing session.
    pub const PAIR_REQUEST: &str = "pair.request";
    /// `hello`: the first request of a session.
    pub const HELLO: &str = "hello";
    /// `panes.list`.
    pub const PANES_LIST: &str = "panes.list";
    /// `panes.subscribe`.
    pub const PANES_SUBSCRIBE: &str = "panes.subscribe";
    /// `panes.unsubscribe`.
    pub const PANES_UNSUBSCRIBE: &str = "panes.unsubscribe";
    /// `terminal.subscribe`.
    pub const TERMINAL_SUBSCRIBE: &str = "terminal.subscribe";
    /// `terminal.unsubscribe`.
    pub const TERMINAL_UNSUBSCRIBE: &str = "terminal.unsubscribe";
    /// `terminal.input`.
    pub const TERMINAL_INPUT: &str = "terminal.input";
    /// `terminal.paste`.
    pub const TERMINAL_PASTE: &str = "terminal.paste";
    /// `device.unpair`.
    pub const DEVICE_UNPAIR: &str = "device.unpair";
}

/// The event names (section 8.5).
pub mod event {
    /// `panes.changed`, host to device.
    pub const PANES_CHANGED: &str = "panes.changed";
    /// `terminal.snapshot`, host to device.
    pub const TERMINAL_SNAPSHOT: &str = "terminal.snapshot";
    /// `terminal.data`, host to device.
    pub const TERMINAL_DATA: &str = "terminal.data";
    /// `terminal.resized`, host to device.
    pub const TERMINAL_RESIZED: &str = "terminal.resized";
    /// `terminal.exit`, host to device.
    pub const TERMINAL_EXIT: &str = "terminal.exit";
    /// `terminal.closed`, host to device.
    pub const TERMINAL_CLOSED: &str = "terminal.closed";
    /// `device.revoked`, host to device. The host closes the session right after it.
    pub const DEVICE_REVOKED: &str = "device.revoked";
    /// `terminal.ack`, device to host.
    pub const TERMINAL_ACK: &str = "terminal.ack";
}

/// The capabilities of `hello` (section 8.2). Each side ignores unknown capabilities.
pub mod caps {
    /// `panes.v1`.
    pub const PANES_V1: &str = "panes.v1";
    /// `terminal.v1`.
    pub const TERMINAL_V1: &str = "terminal.v1";
    /// `terminal.paste.v1`.
    pub const TERMINAL_PASTE_V1: &str = "terminal.paste.v1";
    /// `device.unpair.v1`.
    pub const DEVICE_UNPAIR_V1: &str = "device.unpair.v1";
    /// `agent-status.v1`: the host sends it when `Pane.status` is available.
    pub const AGENT_STATUS_V1: &str = "agent-status.v1";

    /// The capabilities of every version 1 host.
    pub const HOST_V1: [&str; 4] = [PANES_V1, TERMINAL_V1, TERMINAL_PASTE_V1, DEVICE_UNPAIR_V1];

    /// Returns true when `caps` holds `cap`.
    pub fn has(caps: &[String], cap: &str) -> bool {
        caps.iter().any(|item| item == cap)
    }
}

/// Declares a text enum that keeps unknown values in `Other`.
///
/// Make a value with `From<&str>`, so a known text never becomes `Other`.
macro_rules! open_text_enum {
    (
        $(#[$meta:meta])*
        $name:ident {
            $($(#[$variant_meta:meta])* $variant:ident = $text:literal,)*
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub enum $name {
            $($(#[$variant_meta])* $variant,)*
            /// A value that this version does not know. The text is kept.
            Other(String),
        }

        impl $name {
            /// Returns the wire text.
            pub fn as_str(&self) -> &str {
                match self {
                    $($name::$variant => $text,)*
                    $name::Other(text) => text,
                }
            }
        }

        impl From<&str> for $name {
            fn from(text: &str) -> Self {
                match text {
                    $($text => $name::$variant,)*
                    other => $name::Other(other.to_owned()),
                }
            }
        }

        impl From<String> for $name {
            fn from(text: String) -> Self {
                match $name::from(text.as_str()) {
                    $name::Other(_) => $name::Other(text),
                    known => known,
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                String::deserialize(deserializer).map($name::from)
            }
        }
    };
}

open_text_enum!(
    /// An error code of section 8.7.
    ErrorCode {
        /// `bad_request`: the params are not valid.
        BadRequest = "bad_request",
        /// `unknown_method`: the method is unknown or not allowed here.
        UnknownMethod = "unknown_method",
        /// `not_found`: the pane or the subscription does not exist.
        NotFound = "not_found",
        /// `denied`: the user or the host refused.
        Denied = "denied",
        /// `timeout`: no answer in time.
        Timeout = "timeout",
        /// `busy`: the host handles another request of this kind.
        Busy = "busy",
        /// `expired`: the pairing offer ended.
        Expired = "expired",
        /// `unavailable`: the feature is not available now.
        Unavailable = "unavailable",
        /// `incompatible`: the versions or the capabilities do not match.
        Incompatible = "incompatible",
        /// `too_large`: a value is above its limit.
        TooLarge = "too_large",
        /// `internal`: an internal error.
        Internal = "internal",
    }
);

open_text_enum!(
    /// The platform of a device: `ios`, `android`, or other text.
    Platform {
        /// `ios`.
        Ios = "ios",
        /// `android`.
        Android = "android",
    }
);

open_text_enum!(
    /// The kind of a pane: an agent name, `shell`, or other text.
    PaneKind {
        /// `claude`.
        Claude = "claude",
        /// `codex`.
        Codex = "codex",
        /// `kimi`.
        Kimi = "kimi",
        /// `glm`.
        Glm = "glm",
        /// `grok`.
        Grok = "grok",
        /// `gemini`.
        Gemini = "gemini",
        /// `shell`.
        Shell = "shell",
    }
);

open_text_enum!(
    /// The state of an agent (section 8.6).
    ///
    /// The contract marks `AgentStatus` as provisional, so an unknown state is kept.
    /// A receiver shows an unknown state as a neutral state.
    AgentState {
        /// `working`.
        Working = "working",
        /// `done`.
        Done = "done",
        /// `needsInput`.
        NeedsInput = "needsInput",
        /// `needsApproval`.
        NeedsApproval = "needsApproval",
        /// `idle`.
        Idle = "idle",
        /// `exited`.
        Exited = "exited",
        /// `failed`.
        Failed = "failed",
        /// `interrupted`.
        Interrupted = "interrupted",
    }
);

/// The error of a failed request: `{ "code": text, "msg": text }`.
///
/// The host writes `msg` for people. It never holds secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{code}: {msg}")]
pub struct RpcError {
    /// The error code.
    pub code: ErrorCode,
    /// The error text. A missing `msg` decodes as empty text.
    #[serde(default)]
    pub msg: String,
}

impl RpcError {
    /// Makes an error.
    pub fn new(code: ErrorCode, msg: impl Into<String>) -> RpcError {
        RpcError {
            code,
            msg: msg.into(),
        }
    }

    /// Makes a `bad_request` error.
    pub fn bad_request(msg: impl Into<String>) -> RpcError {
        RpcError::new(ErrorCode::BadRequest, msg)
    }

    /// Makes the `unknown_method` error.
    pub fn unknown_method() -> RpcError {
        RpcError::new(ErrorCode::UnknownMethod, "unknown method")
    }

    /// Makes a `too_large` error.
    pub fn too_large(msg: impl Into<String>) -> RpcError {
        RpcError::new(ErrorCode::TooLarge, msg)
    }
}

/// The empty map `{}`: the params of methods without params, and the result `{}`.
///
/// It decodes from any map and ignores the fields.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Empty {}

/// An application message.
#[derive(Clone, PartialEq)]
pub enum Message {
    /// A request (`t: "q"`).
    Request(Request),
    /// A response (`t: "r"`).
    Response(Response),
    /// An event (`t: "e"`).
    Event(Event),
}

/// A request with its params as a CBOR value.
#[derive(Clone, PartialEq)]
pub struct Request {
    /// The request id.
    pub id: RequestId,
    /// The method name (`m`).
    pub method: String,
    /// The params (`p`). A missing `p` decodes as the empty map.
    pub params: Value,
}

/// A response with its result as a CBOR value.
#[derive(Clone, PartialEq)]
pub struct Response {
    /// The request id.
    pub id: RequestId,
    /// The result value (`v`), or the error (`err`).
    /// A missing or null `v` decodes as `Value::Null`.
    pub outcome: std::result::Result<Value, RpcError>,
}

/// An event with its payload as a CBOR value.
#[derive(Clone, PartialEq)]
pub struct Event {
    /// The event name (`m`).
    pub name: String,
    /// The payload (`p`). A missing `p` decodes as the empty map.
    pub payload: Value,
}

fn empty_map() -> Value {
    Value::Map(Vec::new())
}

fn to_cbor<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    ciborium::into_writer(value, &mut out).map_err(|_| Error::Cbor("application message"))?;
    Ok(out)
}

#[derive(Serialize)]
struct WireRequest<'a, P> {
    t: &'static str,
    id: RequestId,
    m: &'a str,
    p: &'a P,
}

#[derive(Serialize)]
struct WireResult<'a, V> {
    t: &'static str,
    id: RequestId,
    ok: bool,
    v: &'a V,
}

#[derive(Serialize)]
struct WireError<'a> {
    t: &'static str,
    id: RequestId,
    ok: bool,
    err: &'a RpcError,
}

#[derive(Serialize)]
struct WireEvent<'a, P> {
    t: &'static str,
    m: &'a str,
    p: &'a P,
}

/// Writes a request. `params` must serialize as a map.
pub fn encode_request<P: Serialize>(id: RequestId, method: &str, params: &P) -> Result<Vec<u8>> {
    to_cbor(&WireRequest {
        t: "q",
        id,
        m: method,
        p: params,
    })
}

/// Writes a request of a typed method.
pub fn encode_call<M: Method>(id: RequestId, params: &M::Params) -> Result<Vec<u8>> {
    encode_request(id, M::NAME, params)
}

/// Writes a successful response.
pub fn encode_response<V: Serialize>(id: RequestId, value: &V) -> Result<Vec<u8>> {
    to_cbor(&WireResult {
        t: "r",
        id,
        ok: true,
        v: value,
    })
}

/// Writes an error response.
pub fn encode_error(id: RequestId, error: &RpcError) -> Result<Vec<u8>> {
    to_cbor(&WireError {
        t: "r",
        id,
        ok: false,
        err: error,
    })
}

/// Writes an event. `payload` must serialize as a map.
pub fn encode_event<P: Serialize>(name: &str, payload: &P) -> Result<Vec<u8>> {
    to_cbor(&WireEvent {
        t: "e",
        m: name,
        p: payload,
    })
}

impl Message {
    /// Writes the message as CBOR.
    pub fn encode(&self) -> Result<Vec<u8>> {
        match self {
            Message::Request(request) => {
                encode_request(request.id, &request.method, &request.params)
            }
            Message::Response(response) => match &response.outcome {
                Ok(value) => encode_response(response.id, value),
                Err(error) => encode_error(response.id, error),
            },
            Message::Event(event) => encode_event(&event.name, &event.payload),
        }
    }

    /// Reads a message.
    ///
    /// Unknown fields are ignored, also the fields of the other message types,
    /// for example `id` in an event. A key that is not text is an unknown field.
    /// A missing or null `p` is the empty map.
    ///
    /// An unknown `t`, a missing or wrong required field, a request id of 0,
    /// a repeated field, or bytes after the map fail with [`Error::Cbor`].
    /// The connection then ends with a protocol error.
    pub fn decode(bytes: &[u8]) -> Result<Message> {
        let mut reader = bytes;
        let value: Value =
            ciborium::de::from_reader_with_recursion_limit(&mut reader, RECURSION_LIMIT)
                .map_err(|_| Error::Cbor("application message"))?;
        if !reader.is_empty() {
            return Err(Error::Cbor("bytes after the application message"));
        }
        let Value::Map(entries) = value else {
            return Err(Error::Cbor("the application message is not a map"));
        };
        let mut fields = EnvelopeFields::default();
        for (key, value) in entries {
            if let Value::Text(key) = key {
                fields.set(&key, value)?;
            }
        }
        let Some(Value::Text(kind)) = fields.t.take() else {
            return Err(Error::Cbor("message without a valid t"));
        };
        match kind.as_str() {
            "q" => Ok(Message::Request(Request {
                id: fields.request_id()?,
                method: fields.name()?,
                params: fields.params(),
            })),
            "r" => {
                let id = fields.request_id()?;
                let outcome = match fields.ok.take() {
                    Some(Value::Bool(true)) => Ok(fields.v.take().unwrap_or(Value::Null)),
                    Some(Value::Bool(false)) => Err(fields
                        .err
                        .take()
                        .and_then(|err| err.deserialized::<RpcError>().ok())
                        .ok_or(Error::Cbor("error response without a valid err"))?),
                    _ => return Err(Error::Cbor("response without a valid ok")),
                };
                Ok(Message::Response(Response { id, outcome }))
            }
            "e" => Ok(Message::Event(Event {
                name: fields.name()?,
                payload: fields.params(),
            })),
            _ => Err(Error::Cbor("unknown message type")),
        }
    }
}

/// The envelope fields of an application message, before the message type is known.
#[derive(Default)]
struct EnvelopeFields {
    t: Option<Value>,
    id: Option<Value>,
    m: Option<Value>,
    p: Option<Value>,
    ok: Option<Value>,
    v: Option<Value>,
    err: Option<Value>,
}

impl EnvelopeFields {
    /// Keeps the value of an envelope field. It ignores other keys and rejects a repeated field.
    fn set(&mut self, key: &str, value: Value) -> Result<()> {
        let slot = match key {
            "t" => &mut self.t,
            "id" => &mut self.id,
            "m" => &mut self.m,
            "p" => &mut self.p,
            "ok" => &mut self.ok,
            "v" => &mut self.v,
            "err" => &mut self.err,
            _ => return Ok(()),
        };
        if slot.replace(value).is_some() {
            return Err(Error::Cbor(
                "a field of the application message is repeated",
            ));
        }
        Ok(())
    }

    /// Takes `id`: a `u32` that is not zero.
    fn request_id(&mut self) -> Result<RequestId> {
        match self.id.take() {
            Some(Value::Integer(id)) => u32::try_from(id)
                .ok()
                .and_then(RequestId::new)
                .ok_or(Error::Cbor("invalid request id")),
            _ => Err(Error::Cbor("message without a request id")),
        }
    }

    /// Takes `m`: the method name of a request or the name of an event.
    fn name(&mut self) -> Result<String> {
        match self.m.take() {
            Some(Value::Text(name)) => Ok(name),
            _ => Err(Error::Cbor("message without a valid name")),
        }
    }

    /// Takes `p`. A missing or null `p` is the empty map.
    fn params(&mut self) -> Value {
        match self.p.take() {
            None | Some(Value::Null) => empty_map(),
            Some(value) => value,
        }
    }
}

impl Request {
    /// Decodes the params. A failure gives the error `bad_request`.
    pub fn params<P: DeserializeOwned>(&self) -> std::result::Result<P, RpcError> {
        self.params
            .deserialized()
            .map_err(|_| RpcError::bad_request("invalid params"))
    }

    /// Overwrites the bytes and the text of the params with zeros.
    ///
    /// The host calls it after it read `pair.request`, because the params hold the relay token.
    #[cfg_attr(not(feature = "host"), allow(dead_code))]
    pub(crate) fn wipe_params(&mut self) {
        wipe_value(&mut self.params);
    }
}

/// Overwrites every byte string and every text in a CBOR value with zeros.
fn wipe_value(value: &mut Value) {
    use zeroize::Zeroize;
    match value {
        Value::Bytes(bytes) => bytes.zeroize(),
        Value::Text(text) => text.zeroize(),
        Value::Array(items) => items.iter_mut().for_each(wipe_value),
        Value::Map(entries) => {
            for (key, value) in entries.iter_mut() {
                wipe_value(key);
                wipe_value(value);
            }
        }
        Value::Tag(_, inner) => wipe_value(inner),
        _ => {}
    }
}

impl Response {
    /// Decodes the result.
    ///
    /// The outer error means that the value does not decode: a protocol error.
    /// The inner error is the error that the peer sent.
    pub fn result<V: DeserializeOwned>(&self) -> Result<std::result::Result<V, RpcError>> {
        match &self.outcome {
            Ok(value) => value
                .deserialized()
                .map(Ok)
                .map_err(|_| Error::Cbor("response value")),
            Err(error) => Ok(Err(error.clone())),
        }
    }

    /// Decodes the result of a typed method.
    pub fn decode_result<M: Method>(&self) -> Result<std::result::Result<M::Result, RpcError>> {
        self.result()
    }
}

impl Event {
    /// Decodes the payload.
    pub fn payload<P: DeserializeOwned>(&self) -> Result<P> {
        self.payload
            .deserialized()
            .map_err(|_| Error::Cbor("event payload"))
    }
}

impl fmt::Debug for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Message::Request(request) => request.fmt(f),
            Message::Response(response) => response.fmt(f),
            Message::Event(event) => event.fmt(f),
        }
    }
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Request")
            .field("id", &self.id)
            .field("method", &self.method)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("Response");
        debug.field("id", &self.id);
        match &self.outcome {
            Ok(_) => debug.field("ok", &true),
            Err(error) => debug.field("ok", &false).field("error", &error.code),
        };
        debug.finish_non_exhaustive()
    }
}

impl fmt::Debug for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Event")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// A method: its name, its params type, and its result type.
pub trait Method {
    /// The method name.
    const NAME: &'static str;
    /// The params type.
    type Params: Serialize + DeserializeOwned;
    /// The result type.
    type Result: Serialize + DeserializeOwned;
}

macro_rules! methods {
    ($($(#[$meta:meta])* $name:ident = $text:path, $params:ty => $result:ty;)*) => {
        $(
            $(#[$meta])*
            #[derive(Debug)]
            pub enum $name {}

            impl Method for $name {
                const NAME: &'static str = $text;
                type Params = $params;
                type Result = $result;
            }
        )*
    };
}

methods! {
    /// `pair.request` (section 8.3).
    PairRequest = method::PAIR_REQUEST, PairRequestParams => PairRequestResult;
    /// `hello`.
    Hello = method::HELLO, HelloParams => HelloResult;
    /// `panes.list`.
    PanesList = method::PANES_LIST, Empty => PaneList;
    /// `panes.subscribe`. Then `panes.changed` events follow.
    PanesSubscribe = method::PANES_SUBSCRIBE, Empty => PaneList;
    /// `panes.unsubscribe`.
    PanesUnsubscribe = method::PANES_UNSUBSCRIBE, Empty => Empty;
    /// `terminal.subscribe`. Then terminal events follow.
    TerminalSubscribe = method::TERMINAL_SUBSCRIBE, TerminalSubscribeParams => TerminalSubscribeResult;
    /// `terminal.unsubscribe`.
    TerminalUnsubscribe = method::TERMINAL_UNSUBSCRIBE, TerminalUnsubscribeParams => Empty;
    /// `terminal.input`.
    TerminalInput = method::TERMINAL_INPUT, TerminalInputParams => Empty;
    /// `terminal.paste`.
    TerminalPaste = method::TERMINAL_PASTE, TerminalPasteParams => Empty;
    /// `device.unpair`. Then the host revokes the device and closes with `4401`.
    DeviceUnpair = method::DEVICE_UNPAIR, Empty => Empty;
}

/// The params of `pair.request`.
///
/// `Debug` does not show the token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairRequestParams {
    /// The device name, 1 to 64 characters.
    pub name: String,
    /// The platform.
    pub platform: Platform,
    /// The device model, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The new relay token of the device (a CBOR byte string of 32 bytes).
    pub token: RelayToken,
    /// The app name and version.
    pub app: String,
}

impl PairRequestParams {
    /// Reads the only request of a pairing session (section 8.3).
    ///
    /// Another method gives `unknown_method`. Params that do not decode,
    /// or a field that breaks a limit of [`PairRequestParams::validate`], give `bad_request`.
    pub fn parse(request: &Request) -> std::result::Result<PairRequestParams, RpcError> {
        if request.method != method::PAIR_REQUEST {
            return Err(RpcError::unknown_method());
        }
        let params: PairRequestParams = request.params()?;
        params.validate()?;
        Ok(params)
    }

    /// Checks the limits of section 8.3:
    ///
    /// - the name has 1 to 64 characters,
    /// - the platform has 1 to 32 characters,
    /// - the model, if present, has at most 64 characters.
    pub fn validate(&self) -> std::result::Result<(), RpcError> {
        let name = self.name.chars().count();
        if !(1..=MAX_DEVICE_NAME_CHARS).contains(&name) {
            return Err(RpcError::bad_request(
                "the device name must have 1 to 64 characters",
            ));
        }
        let platform = self.platform.as_str().chars().count();
        if !(1..=MAX_PLATFORM_CHARS).contains(&platform) {
            return Err(RpcError::bad_request(
                "the platform must have 1 to 32 characters",
            ));
        }
        if self
            .model
            .as_ref()
            .is_some_and(|model| model.chars().count() > MAX_MODEL_CHARS)
        {
            return Err(RpcError::bad_request(
                "the model must have at most 64 characters",
            ));
        }
        Ok(())
    }
}

/// The result of `pair.request`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairRequestResult {
    /// The device id that the host made.
    pub device: DeviceId,
    /// The host name.
    pub host: String,
}

/// The params of `hello`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloParams {
    /// The app name and version.
    pub app: String,
    /// The platform.
    pub platform: Platform,
    /// The capabilities of the device.
    pub caps: Vec<String>,
}

/// The host part of the `hello` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostInfo {
    /// The host name.
    pub name: String,
    /// The app name and version.
    pub app: String,
    /// The operating system.
    pub os: String,
}

/// The result of `hello`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloResult {
    /// The host.
    pub host: HostInfo,
    /// The device id of this device.
    pub device: DeviceId,
    /// The capabilities of the host.
    pub caps: Vec<String>,
    /// The host time in epoch milliseconds.
    pub time: u64,
}

/// The pane list: the result of `panes.list` and `panes.subscribe`,
/// and the payload of `panes.changed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneList {
    /// The panes.
    pub panes: Vec<Pane>,
    /// The revision of the list.
    pub rev: u64,
}

/// The params of `terminal.subscribe`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSubscribeParams {
    /// The terminal session id of the pane.
    pub session: String,
    /// The last applied sequence number, after a reconnect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_seq: Option<u64>,
}

/// The result of `terminal.subscribe`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSubscribeResult {
    /// The subscription id.
    pub sub: u32,
    /// The columns of the PTY.
    pub cols: u16,
    /// The rows of the PTY.
    pub rows: u16,
    /// The sequence number of the last chunk.
    pub seq: u64,
}

/// The params of `terminal.unsubscribe`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalUnsubscribeParams {
    /// The subscription id.
    pub sub: u32,
}

/// The params of `terminal.input`.
///
/// `Debug` shows only the data length, because input can hold passwords.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalInputParams {
    /// The terminal session id of the pane.
    pub session: String,
    /// The bytes to write (a CBOR byte string of at most 4 KiB).
    #[serde(with = "serde_bytes")]
    pub data: Vec<u8>,
}

impl TerminalInputParams {
    /// Checks the 4 KiB limit. A larger input gives `too_large`.
    pub fn validate(&self) -> std::result::Result<(), RpcError> {
        if self.data.len() > MAX_TERMINAL_INPUT {
            Err(RpcError::too_large("terminal input is larger than 4 KiB"))
        } else {
            Ok(())
        }
    }
}

impl fmt::Debug for TerminalInputParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TerminalInputParams")
            .field("session", &self.session)
            .field("data_len", &self.data.len())
            .finish()
    }
}

/// The params of `terminal.paste`.
///
/// `Debug` shows only the text length.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalPasteParams {
    /// The terminal session id of the pane.
    pub session: String,
    /// The text to paste (at most 64 KiB of UTF-8).
    pub text: String,
    /// When true, the host writes `\r` after the text.
    pub submit: bool,
}

impl TerminalPasteParams {
    /// Checks the 64 KiB limit. A larger text gives `too_large`.
    pub fn validate(&self) -> std::result::Result<(), RpcError> {
        if self.text.len() > MAX_TERMINAL_PASTE {
            Err(RpcError::too_large("paste text is larger than 64 KiB"))
        } else {
            Ok(())
        }
    }
}

impl fmt::Debug for TerminalPasteParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TerminalPasteParams")
            .field("session", &self.session)
            .field("text_len", &self.text.len())
            .field("submit", &self.submit)
            .finish()
    }
}

/// The state of a pane process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PaneState {
    /// `running`.
    Running,
    /// `exited`.
    Exited,
}

/// A terminal pane of the host (section 8.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pane {
    /// The terminal session id.
    pub session: String,
    /// The title.
    pub title: String,
    /// The kind: an agent name, `shell`, or other text.
    pub kind: PaneKind,
    /// The project name, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// The working directory, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// The columns of the PTY.
    pub cols: u16,
    /// The rows of the PTY.
    pub rows: u16,
    /// The process state.
    pub state: PaneState,
    /// The exit code, after the process exited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// The time of the last output in epoch milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_output_at: Option<u64>,
    /// The agent status. It is present with the capability `agent-status.v1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<AgentStatus>,
}

/// The status of an agent (section 8.6). The contract marks it as provisional.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatus {
    /// The state.
    pub state: AgentState,
    /// The reason for the state, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The time of the last change in epoch milliseconds.
    pub updated_at: u64,
    /// True when the user did not see the last change.
    pub unread: bool,
    /// The open prompt. It encodes as `null` when there is none.
    #[serde(default)]
    pub prompt: Option<Prompt>,
}

/// A prompt that waits for the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawPrompt", into = "RawPrompt")]
pub enum Prompt {
    /// `{ kind: "approval", toolName, command?, filePath?, message? }`.
    Approval {
        /// The tool that asks for approval.
        tool_name: String,
        /// The command, if the tool runs one.
        command: Option<String>,
        /// The file, if the tool changes one.
        file_path: Option<String>,
        /// The message, if any.
        message: Option<String>,
    },
    /// `{ kind: "question", text, options? }`.
    Question {
        /// The question.
        text: String,
        /// The answer options, if any.
        options: Option<Vec<String>>,
    },
    /// A prompt kind that this version does not know. The kind text is kept.
    Other {
        /// The kind text.
        kind: String,
    },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPrompt {
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    file_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    options: Option<Vec<String>>,
}

impl TryFrom<RawPrompt> for Prompt {
    type Error = &'static str;

    fn try_from(raw: RawPrompt) -> std::result::Result<Self, Self::Error> {
        match raw.kind.as_str() {
            "approval" => Ok(Prompt::Approval {
                tool_name: raw.tool_name.ok_or("an approval prompt needs toolName")?,
                command: raw.command,
                file_path: raw.file_path,
                message: raw.message,
            }),
            "question" => Ok(Prompt::Question {
                text: raw.text.ok_or("a question prompt needs text")?,
                options: raw.options,
            }),
            _ => Ok(Prompt::Other { kind: raw.kind }),
        }
    }
}

impl From<Prompt> for RawPrompt {
    fn from(prompt: Prompt) -> Self {
        let empty = RawPrompt {
            kind: String::new(),
            tool_name: None,
            command: None,
            file_path: None,
            message: None,
            text: None,
            options: None,
        };
        match prompt {
            Prompt::Approval {
                tool_name,
                command,
                file_path,
                message,
            } => RawPrompt {
                kind: "approval".to_owned(),
                tool_name: Some(tool_name),
                command,
                file_path,
                message,
                ..empty
            },
            Prompt::Question { text, options } => RawPrompt {
                kind: "question".to_owned(),
                text: Some(text),
                options,
                ..empty
            },
            Prompt::Other { kind } => RawPrompt { kind, ..empty },
        }
    }
}

/// The payload of `terminal.snapshot`: the full ring of a pane.
///
/// The device resets its terminal before it writes a snapshot.
/// `Debug` shows only the data length.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSnapshot {
    /// The subscription id.
    pub sub: u32,
    /// The sequence number of the last chunk in the snapshot.
    pub seq: u64,
    /// The columns of the PTY.
    pub cols: u16,
    /// The rows of the PTY.
    pub rows: u16,
    /// The terminal output (a CBOR byte string).
    #[serde(with = "serde_bytes")]
    pub data: Vec<u8>,
}

/// The payload of `terminal.data`: new output.
///
/// `Debug` shows only the data length.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalData {
    /// The subscription id.
    pub sub: u32,
    /// The sequence number of the last chunk in the event.
    pub seq: u64,
    /// The terminal output (a CBOR byte string).
    #[serde(with = "serde_bytes")]
    pub data: Vec<u8>,
}

impl fmt::Debug for TerminalSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TerminalSnapshot")
            .field("sub", &self.sub)
            .field("seq", &self.seq)
            .field("cols", &self.cols)
            .field("rows", &self.rows)
            .field("data_len", &self.data.len())
            .finish()
    }
}

impl fmt::Debug for TerminalData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TerminalData")
            .field("sub", &self.sub)
            .field("seq", &self.seq)
            .field("data_len", &self.data.len())
            .finish()
    }
}

/// The payload of `terminal.resized`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalResized {
    /// The subscription id.
    pub sub: u32,
    /// The new columns.
    pub cols: u16,
    /// The new rows.
    pub rows: u16,
}

/// The payload of `terminal.exit`. The subscription stays open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalExit {
    /// The subscription id.
    pub sub: u32,
    /// The exit code, if the process exited normally.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<i32>,
    /// The signal name, if a signal stopped the process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<String>,
}

/// The payload of `terminal.closed`. The subscription ends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalClosed {
    /// The subscription id.
    pub sub: u32,
    /// The reason.
    pub reason: String,
}

/// The payload of `terminal.ack`: the last applied sequence number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalAck {
    /// The subscription id.
    pub sub: u32,
    /// The last applied sequence number.
    pub seq: u64,
}

/// A request of a session, as the host dispatches it (section 8.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionRequest {
    /// `hello`.
    Hello(HelloParams),
    /// `panes.list`.
    PanesList,
    /// `panes.subscribe`.
    PanesSubscribe,
    /// `panes.unsubscribe`.
    PanesUnsubscribe,
    /// `terminal.subscribe`.
    TerminalSubscribe(TerminalSubscribeParams),
    /// `terminal.unsubscribe`.
    TerminalUnsubscribe(TerminalUnsubscribeParams),
    /// `terminal.input`.
    TerminalInput(TerminalInputParams),
    /// `terminal.paste`.
    TerminalPaste(TerminalPasteParams),
    /// `device.unpair`.
    DeviceUnpair,
}

impl SessionRequest {
    /// Reads a session request.
    ///
    /// An unknown method, and `pair.request`, give `unknown_method`.
    /// Params that do not decode give `bad_request`.
    /// Input or paste above its limit gives `too_large`.
    pub fn parse(request: &Request) -> std::result::Result<SessionRequest, RpcError> {
        let parsed = match request.method.as_str() {
            method::HELLO => SessionRequest::Hello(request.params()?),
            method::PANES_LIST => {
                request.params::<Empty>()?;
                SessionRequest::PanesList
            }
            method::PANES_SUBSCRIBE => {
                request.params::<Empty>()?;
                SessionRequest::PanesSubscribe
            }
            method::PANES_UNSUBSCRIBE => {
                request.params::<Empty>()?;
                SessionRequest::PanesUnsubscribe
            }
            method::TERMINAL_SUBSCRIBE => SessionRequest::TerminalSubscribe(request.params()?),
            method::TERMINAL_UNSUBSCRIBE => SessionRequest::TerminalUnsubscribe(request.params()?),
            method::TERMINAL_INPUT => {
                let params: TerminalInputParams = request.params()?;
                params.validate()?;
                SessionRequest::TerminalInput(params)
            }
            method::TERMINAL_PASTE => {
                let params: TerminalPasteParams = request.params()?;
                params.validate()?;
                SessionRequest::TerminalPaste(params)
            }
            method::DEVICE_UNPAIR => {
                request.params::<Empty>()?;
                SessionRequest::DeviceUnpair
            }
            _ => return Err(RpcError::unknown_method()),
        };
        Ok(parsed)
    }

    /// Returns the method name.
    pub fn method(&self) -> &'static str {
        match self {
            SessionRequest::Hello(_) => method::HELLO,
            SessionRequest::PanesList => method::PANES_LIST,
            SessionRequest::PanesSubscribe => method::PANES_SUBSCRIBE,
            SessionRequest::PanesUnsubscribe => method::PANES_UNSUBSCRIBE,
            SessionRequest::TerminalSubscribe(_) => method::TERMINAL_SUBSCRIBE,
            SessionRequest::TerminalUnsubscribe(_) => method::TERMINAL_UNSUBSCRIBE,
            SessionRequest::TerminalInput(_) => method::TERMINAL_INPUT,
            SessionRequest::TerminalPaste(_) => method::TERMINAL_PASTE,
            SessionRequest::DeviceUnpair => method::DEVICE_UNPAIR,
        }
    }

    /// Writes the request with the id `id`.
    pub fn encode(&self, id: RequestId) -> Result<Vec<u8>> {
        let method = self.method();
        match self {
            SessionRequest::Hello(params) => encode_request(id, method, params),
            SessionRequest::TerminalSubscribe(params) => encode_request(id, method, params),
            SessionRequest::TerminalUnsubscribe(params) => encode_request(id, method, params),
            SessionRequest::TerminalInput(params) => encode_request(id, method, params),
            SessionRequest::TerminalPaste(params) => encode_request(id, method, params),
            SessionRequest::PanesList
            | SessionRequest::PanesSubscribe
            | SessionRequest::PanesUnsubscribe
            | SessionRequest::DeviceUnpair => encode_request(id, method, &Empty {}),
        }
    }
}

/// An event from the host to the device (section 8.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEvent {
    /// `panes.changed`.
    PanesChanged(PaneList),
    /// `terminal.snapshot`.
    TerminalSnapshot(TerminalSnapshot),
    /// `terminal.data`.
    TerminalData(TerminalData),
    /// `terminal.resized`.
    TerminalResized(TerminalResized),
    /// `terminal.exit`.
    TerminalExit(TerminalExit),
    /// `terminal.closed`.
    TerminalClosed(TerminalClosed),
    /// `device.revoked`: the host revoked this device and closes the session next.
    /// Only after this event does the device show "Access revoked" and delete its keys.
    DeviceRevoked,
}

impl HostEvent {
    /// Reads a host event. An unknown event name gives `Ok(None)`: the device ignores it.
    /// A known event with a payload that does not decode fails.
    pub fn parse(event: &Event) -> Result<Option<HostEvent>> {
        let parsed = match event.name.as_str() {
            event::PANES_CHANGED => HostEvent::PanesChanged(event.payload()?),
            event::TERMINAL_SNAPSHOT => HostEvent::TerminalSnapshot(event.payload()?),
            event::TERMINAL_DATA => HostEvent::TerminalData(event.payload()?),
            event::TERMINAL_RESIZED => HostEvent::TerminalResized(event.payload()?),
            event::TERMINAL_EXIT => HostEvent::TerminalExit(event.payload()?),
            event::TERMINAL_CLOSED => HostEvent::TerminalClosed(event.payload()?),
            // The event has no payload, so the device ignores the payload.
            // The Noise channel already proves that the host sent the event.
            event::DEVICE_REVOKED => HostEvent::DeviceRevoked,
            _ => return Ok(None),
        };
        Ok(Some(parsed))
    }

    /// Returns the event name.
    pub fn name(&self) -> &'static str {
        match self {
            HostEvent::PanesChanged(_) => event::PANES_CHANGED,
            HostEvent::TerminalSnapshot(_) => event::TERMINAL_SNAPSHOT,
            HostEvent::TerminalData(_) => event::TERMINAL_DATA,
            HostEvent::TerminalResized(_) => event::TERMINAL_RESIZED,
            HostEvent::TerminalExit(_) => event::TERMINAL_EXIT,
            HostEvent::TerminalClosed(_) => event::TERMINAL_CLOSED,
            HostEvent::DeviceRevoked => event::DEVICE_REVOKED,
        }
    }

    /// Writes the event.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let name = self.name();
        match self {
            HostEvent::PanesChanged(payload) => encode_event(name, payload),
            HostEvent::TerminalSnapshot(payload) => encode_event(name, payload),
            HostEvent::TerminalData(payload) => encode_event(name, payload),
            HostEvent::TerminalResized(payload) => encode_event(name, payload),
            HostEvent::TerminalExit(payload) => encode_event(name, payload),
            HostEvent::TerminalClosed(payload) => encode_event(name, payload),
            HostEvent::DeviceRevoked => encode_event(name, &Empty {}),
        }
    }
}

/// An event from the device to the host (section 8.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceEvent {
    /// `terminal.ack`.
    TerminalAck(TerminalAck),
}

impl DeviceEvent {
    /// Reads a device event. An unknown event name gives `Ok(None)`: the host ignores it.
    pub fn parse(event: &Event) -> Result<Option<DeviceEvent>> {
        match event.name.as_str() {
            event::TERMINAL_ACK => Ok(Some(DeviceEvent::TerminalAck(event.payload()?))),
            _ => Ok(None),
        }
    }

    /// Returns the event name.
    pub fn name(&self) -> &'static str {
        match self {
            DeviceEvent::TerminalAck(_) => event::TERMINAL_ACK,
        }
    }

    /// Writes the event.
    pub fn encode(&self) -> Result<Vec<u8>> {
        match self {
            DeviceEvent::TerminalAck(payload) => encode_event(self.name(), payload),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: u32) -> RequestId {
        RequestId::new(value).unwrap()
    }

    fn decode_request(bytes: &[u8]) -> Request {
        match Message::decode(bytes).unwrap() {
            Message::Request(request) => request,
            other => panic!("not a request: {other:?}"),
        }
    }

    fn decode_response(bytes: &[u8]) -> Response {
        match Message::decode(bytes).unwrap() {
            Message::Response(response) => response,
            other => panic!("not a response: {other:?}"),
        }
    }

    fn decode_event(bytes: &[u8]) -> Event {
        match Message::decode(bytes).unwrap() {
            Message::Event(event) => event,
            other => panic!("not an event: {other:?}"),
        }
    }

    /// Encodes a CBOR value, so a test can build messages with any fields.
    fn cbor(value: &Value) -> Vec<u8> {
        let mut out = Vec::new();
        ciborium::into_writer(value, &mut out).unwrap();
        out
    }

    fn text(value: &str) -> Value {
        Value::Text(value.to_owned())
    }

    fn map(entries: Vec<(&str, Value)>) -> Value {
        Value::Map(
            entries
                .into_iter()
                .map(|(key, value)| (text(key), value))
                .collect(),
        )
    }

    fn sample_pane() -> Pane {
        Pane {
            session: "term-1".to_owned(),
            title: "claude: fix tests".to_owned(),
            kind: PaneKind::Claude,
            project: Some("halv".to_owned()),
            cwd: Some("/Users/me/halv".to_owned()),
            cols: 120,
            rows: 40,
            state: PaneState::Running,
            exit_code: None,
            last_output_at: Some(1_760_460_000_000),
            status: Some(AgentStatus {
                state: AgentState::NeedsApproval,
                reason: Some("tool".to_owned()),
                updated_at: 1_760_460_000_100,
                unread: true,
                prompt: Some(Prompt::Approval {
                    tool_name: "Bash".to_owned(),
                    command: Some("cargo test".to_owned()),
                    file_path: None,
                    message: None,
                }),
            }),
        }
    }

    #[test]
    fn request_has_the_exact_cbor_bytes() {
        let bytes = encode_call::<PanesList>(id(1), &Empty {}).unwrap();
        let mut expected = vec![0xa4];
        expected.extend_from_slice(&[0x61, b't', 0x61, b'q']);
        expected.extend_from_slice(&[0x62, b'i', b'd', 0x01]);
        expected.extend_from_slice(&[0x61, b'm', 0x6a]);
        expected.extend_from_slice(b"panes.list");
        expected.extend_from_slice(&[0x61, b'p', 0xa0]);
        assert_eq!(bytes, expected);
    }

    #[test]
    fn envelopes_have_the_contract_fields() {
        let ok = cbor_value(&encode_response(id(7), &Empty {}).unwrap());
        assert_eq!(
            ok,
            map(vec![
                ("t", text("r")),
                ("id", Value::Integer(7.into())),
                ("ok", Value::Bool(true)),
                ("v", Value::Map(vec![])),
            ])
        );
        let error = cbor_value(&encode_error(id(7), &RpcError::unknown_method()).unwrap());
        assert_eq!(
            error,
            map(vec![
                ("t", text("r")),
                ("id", Value::Integer(7.into())),
                ("ok", Value::Bool(false)),
                (
                    "err",
                    map(vec![
                        ("code", text("unknown_method")),
                        ("msg", text("unknown method"))
                    ])
                ),
            ])
        );
        let event = cbor_value(&HostEvent::DeviceRevoked.encode().unwrap());
        assert_eq!(
            event,
            map(vec![
                ("t", text("e")),
                ("m", text("device.revoked")),
                ("p", Value::Map(vec![])),
            ])
        );
    }

    fn cbor_value(bytes: &[u8]) -> Value {
        ciborium::from_reader(bytes).unwrap()
    }

    #[test]
    fn message_round_trips() {
        let messages = [
            Message::Request(Request {
                id: id(3),
                method: "hello".to_owned(),
                params: map(vec![("app", text("x"))]),
            }),
            Message::Response(Response {
                id: id(3),
                outcome: Ok(Value::Integer(5.into())),
            }),
            Message::Response(Response {
                id: id(4),
                outcome: Err(RpcError::new(ErrorCode::Busy, "busy")),
            }),
            Message::Event(Event {
                name: "terminal.ack".to_owned(),
                payload: map(vec![]),
            }),
        ];
        for message in messages {
            let bytes = message.encode().unwrap();
            assert_eq!(Message::decode(&bytes).unwrap(), message);
        }
    }

    #[test]
    fn decode_ignores_unknown_fields() {
        let bytes = cbor(&map(vec![
            ("x", Value::Array(vec![Value::Bool(true)])),
            ("t", text("q")),
            ("id", Value::Integer(9.into())),
            ("m", text("terminal.unsubscribe")),
            (
                "p",
                map(vec![
                    ("sub", Value::Integer(2.into())),
                    ("future", Value::Null),
                ]),
            ),
            ("trace", text("abc")),
        ]));
        let request = decode_request(&bytes);
        assert_eq!(request.id.get(), 9);
        assert_eq!(
            SessionRequest::parse(&request).unwrap(),
            SessionRequest::TerminalUnsubscribe(TerminalUnsubscribeParams { sub: 2 })
        );
    }

    #[test]
    fn decode_accepts_missing_params_and_null_values() {
        let request = decode_request(&cbor(&map(vec![
            ("t", text("q")),
            ("id", Value::Integer(1.into())),
            ("m", text("panes.list")),
        ])));
        assert_eq!(
            SessionRequest::parse(&request).unwrap(),
            SessionRequest::PanesList
        );
        let response = decode_response(&cbor(&map(vec![
            ("t", text("r")),
            ("id", Value::Integer(1.into())),
            ("ok", Value::Bool(true)),
        ])));
        assert_eq!(response.outcome, Ok(Value::Null));
        // A null `p` is the empty map.
        let request = decode_request(&cbor(&map(vec![
            ("t", text("q")),
            ("id", Value::Integer(1.into())),
            ("m", text("panes.subscribe")),
            ("p", Value::Null),
        ])));
        assert_eq!(request.params, Value::Map(vec![]));
        assert_eq!(
            SessionRequest::parse(&request).unwrap(),
            SessionRequest::PanesSubscribe
        );
    }

    #[test]
    fn decode_ignores_fields_of_other_message_types() {
        // An event with the fields of a request and a response.
        let event = decode_event(&cbor(&map(vec![
            ("t", text("e")),
            ("m", text("terminal.ack")),
            (
                "p",
                map(vec![
                    ("sub", Value::Integer(1.into())),
                    ("seq", Value::Integer(2.into())),
                ]),
            ),
            ("id", Value::Integer(0.into())),
            ("ok", text("yes")),
            ("err", Value::Integer(5.into())),
            ("v", Value::Array(vec![])),
        ])));
        assert_eq!(
            DeviceEvent::parse(&event).unwrap(),
            Some(DeviceEvent::TerminalAck(TerminalAck { sub: 1, seq: 2 }))
        );

        // A request with the fields of a response.
        let request = decode_request(&cbor(&map(vec![
            ("t", text("q")),
            ("id", Value::Integer(4.into())),
            ("m", text("panes.list")),
            ("ok", text("yes")),
            ("err", Value::Integer(1.into())),
            ("v", Value::Integer(2.into())),
        ])));
        assert_eq!(
            SessionRequest::parse(&request).unwrap(),
            SessionRequest::PanesList
        );

        // A response with the fields of a request, and a key that is not text.
        let response = decode_response(&cbor(&Value::Map(vec![
            (text("t"), text("r")),
            (text("id"), Value::Integer(4.into())),
            (text("ok"), Value::Bool(true)),
            (text("v"), Value::Integer(9.into())),
            (text("m"), Value::Integer(5.into())),
            (text("p"), text("x")),
            (Value::Integer(0.into()), text("q")),
        ])));
        assert_eq!(response.id.get(), 4);
        assert_eq!(response.outcome, Ok(Value::Integer(9.into())));
    }

    #[test]
    fn decode_rejects_bad_envelopes() {
        let bad = [
            map(vec![("t", text("x"))]),
            map(vec![("id", Value::Integer(1.into()))]),
            map(vec![("t", text("q")), ("m", text("hello"))]),
            map(vec![
                ("t", text("q")),
                ("id", Value::Integer(0.into())),
                ("m", text("hello")),
            ]),
            map(vec![
                ("t", text("q")),
                ("id", Value::Integer((1u64 << 32).into())),
                ("m", text("hello")),
            ]),
            map(vec![("t", text("q")), ("id", Value::Integer(1.into()))]),
            map(vec![("t", text("r")), ("id", Value::Integer(1.into()))]),
            map(vec![
                ("t", text("r")),
                ("id", Value::Integer(1.into())),
                ("ok", Value::Bool(false)),
            ]),
            map(vec![("t", text("e")), ("p", map(vec![]))]),
            map(vec![("t", Value::Integer(1.into())), ("m", text("x"))]),
            map(vec![("t", text("e")), ("m", Value::Integer(1.into()))]),
            map(vec![
                ("t", text("r")),
                ("id", Value::Integer(1.into())),
                ("ok", text("true")),
            ]),
            // An `err` without `code`.
            map(vec![
                ("t", text("r")),
                ("id", Value::Integer(1.into())),
                ("ok", Value::Bool(false)),
                ("err", map(vec![("msg", text("x"))])),
            ]),
            // Integer keys are unknown fields, so the message has no `t`.
            Value::Map(vec![
                (Value::Integer(0.into()), text("q")),
                (Value::Integer(1.into()), Value::Integer(1.into())),
                (Value::Integer(2.into()), text("hello")),
            ]),
            // A repeated field.
            Value::Map(vec![
                (text("t"), text("e")),
                (text("m"), text("x")),
                (text("t"), text("e")),
            ]),
            Value::Array(vec![text("q")]),
            text("q"),
        ];
        for value in bad {
            assert!(Message::decode(&cbor(&value)).is_err(), "{value:?}");
        }
        let mut trailing = encode_call::<PanesList>(id(1), &Empty {}).unwrap();
        trailing.push(0x00);
        assert!(Message::decode(&trailing).is_err());
        let full = encode_call::<PanesList>(id(1), &Empty {}).unwrap();
        assert!(Message::decode(&full[..full.len() - 1]).is_err());
        assert!(Message::decode(&[]).is_err());

        // Deep nesting stops at the recursion limit.
        let mut deep = Value::Null;
        for _ in 0..100 {
            deep = Value::Array(vec![deep]);
        }
        let nested = cbor(&map(vec![("t", text("e")), ("m", text("x")), ("p", deep)]));
        assert!(Message::decode(&nested).is_err());
    }

    #[test]
    fn session_requests_round_trip() {
        let requests = [
            SessionRequest::Hello(HelloParams {
                app: "halv-mobile/0.1.0".to_owned(),
                platform: Platform::Ios,
                caps: vec!["panes.v1".to_owned()],
            }),
            SessionRequest::PanesList,
            SessionRequest::PanesSubscribe,
            SessionRequest::PanesUnsubscribe,
            SessionRequest::TerminalSubscribe(TerminalSubscribeParams {
                session: "term-1".to_owned(),
                from_seq: Some(41),
            }),
            SessionRequest::TerminalSubscribe(TerminalSubscribeParams {
                session: "term-1".to_owned(),
                from_seq: None,
            }),
            SessionRequest::TerminalUnsubscribe(TerminalUnsubscribeParams { sub: 3 }),
            SessionRequest::TerminalInput(TerminalInputParams {
                session: "term-1".to_owned(),
                data: b"ls\r".to_vec(),
            }),
            SessionRequest::TerminalPaste(TerminalPasteParams {
                session: "term-1".to_owned(),
                text: "echo hi\n".to_owned(),
                submit: true,
            }),
            SessionRequest::DeviceUnpair,
        ];
        for (index, request) in requests.into_iter().enumerate() {
            let bytes = request.encode(id(index as u32 + 1)).unwrap();
            let decoded = decode_request(&bytes);
            assert_eq!(decoded.id.get(), index as u32 + 1);
            assert_eq!(decoded.method, request.method());
            assert_eq!(SessionRequest::parse(&decoded).unwrap(), request);
        }
    }

    #[test]
    fn camel_case_fields_on_the_wire() {
        let bytes = SessionRequest::TerminalSubscribe(TerminalSubscribeParams {
            session: "s".to_owned(),
            from_seq: Some(1),
        })
        .encode(id(1))
        .unwrap();
        let params = decode_request(&bytes).params;
        assert_eq!(
            params,
            map(vec![
                ("session", text("s")),
                ("fromSeq", Value::Integer(1.into()))
            ])
        );
        let pane = Value::serialized(&sample_pane()).unwrap();
        let keys: Vec<&str> = pane
            .as_map()
            .unwrap()
            .iter()
            .map(|(key, _)| key.as_text().unwrap())
            .collect();
        assert!(keys.contains(&"lastOutputAt"));
        assert!(!keys.contains(&"exitCode"));
        let status = Value::serialized(sample_pane().status.as_ref().unwrap()).unwrap();
        let status = status.as_map().unwrap();
        assert!(status
            .iter()
            .any(|(key, _)| key.as_text() == Some("updatedAt")));
        let prompt = &status
            .iter()
            .find(|(key, _)| key.as_text() == Some("prompt"))
            .unwrap()
            .1;
        assert_eq!(
            prompt,
            &map(vec![
                ("kind", text("approval")),
                ("toolName", text("Bash")),
                ("command", text("cargo test")),
            ])
        );
    }

    #[test]
    fn byte_fields_are_cbor_byte_strings() {
        let bytes = SessionRequest::TerminalInput(TerminalInputParams {
            session: "s".to_owned(),
            data: vec![0x1b, b'[', b'A'],
        })
        .encode(id(1))
        .unwrap();
        let params = decode_request(&bytes).params;
        let data = params
            .as_map()
            .unwrap()
            .iter()
            .find(|(key, _)| key.as_text() == Some("data"))
            .unwrap();
        assert_eq!(data.1, Value::Bytes(vec![0x1b, b'[', b'A']));

        // An array of integers is not a byte string.
        let request = decode_request(&cbor(&map(vec![
            ("t", text("q")),
            ("id", Value::Integer(1.into())),
            ("m", text("terminal.input")),
            (
                "p",
                map(vec![
                    ("session", text("s")),
                    ("data", Value::Array(vec![Value::Integer(65.into())])),
                ]),
            ),
        ])));
        assert_eq!(
            SessionRequest::parse(&request).unwrap_err().code,
            ErrorCode::BadRequest
        );
    }

    #[test]
    fn session_request_errors() {
        let request = |method: &str, params: Value| Request {
            id: id(1),
            method: method.to_owned(),
            params,
        };
        let code = |request: Request| SessionRequest::parse(&request).unwrap_err().code;

        assert_eq!(
            code(request("panes.fly", map(vec![]))),
            ErrorCode::UnknownMethod
        );
        assert_eq!(
            code(request("pair.request", map(vec![]))),
            ErrorCode::UnknownMethod
        );
        assert_eq!(code(request("hello", map(vec![]))), ErrorCode::BadRequest);
        assert_eq!(
            code(request("panes.list", text("x"))),
            ErrorCode::BadRequest
        );
        assert_eq!(
            code(request(
                "terminal.unsubscribe",
                map(vec![("sub", Value::Integer((-1).into()))])
            )),
            ErrorCode::BadRequest
        );

        let input = |len: usize| {
            request(
                "terminal.input",
                Value::serialized(&TerminalInputParams {
                    session: "s".to_owned(),
                    data: vec![b'a'; len],
                })
                .unwrap(),
            )
        };
        assert!(SessionRequest::parse(&input(MAX_TERMINAL_INPUT)).is_ok());
        assert_eq!(code(input(MAX_TERMINAL_INPUT + 1)), ErrorCode::TooLarge);

        let paste = |len: usize| {
            request(
                "terminal.paste",
                Value::serialized(&TerminalPasteParams {
                    session: "s".to_owned(),
                    text: "p".repeat(len),
                    submit: false,
                })
                .unwrap(),
            )
        };
        assert!(SessionRequest::parse(&paste(MAX_TERMINAL_PASTE)).is_ok());
        assert_eq!(code(paste(MAX_TERMINAL_PASTE + 1)), ErrorCode::TooLarge);
    }

    fn pair_params(name: &str) -> PairRequestParams {
        PairRequestParams {
            name: name.to_owned(),
            platform: Platform::Android,
            model: Some("Pixel 9".to_owned()),
            token: RelayToken::from_bytes([0x5a; 32]),
            app: "halv-mobile/0.1.0".to_owned(),
        }
    }

    #[test]
    fn pair_request_round_trip() {
        let bytes = encode_call::<PairRequest>(id(1), &pair_params("Ana's phone")).unwrap();
        let request = decode_request(&bytes);
        let params = PairRequestParams::parse(&request).unwrap();
        assert_eq!(params.name, "Ana's phone");
        assert_eq!(params.platform, Platform::Android);
        assert_eq!(params.model.as_deref(), Some("Pixel 9"));
        assert_eq!(params.token.as_bytes(), &[0x5a; 32]);
        assert_eq!(params.app, "halv-mobile/0.1.0");
        // The token is a CBOR byte string of 32 bytes.
        let token = request
            .params
            .as_map()
            .unwrap()
            .iter()
            .find(|(key, _)| key.as_text() == Some("token"))
            .unwrap()
            .1
            .clone();
        assert_eq!(token, Value::Bytes(vec![0x5a; 32]));

        // The host wipes the params after it read them, so no token copy stays in memory.
        let mut request = request;
        request.wipe_params();
        let wiped = request.params.as_map().unwrap();
        assert!(wiped.iter().all(|(key, value)| match (key, value) {
            (Value::Text(key), Value::Bytes(bytes)) => {
                key.bytes().all(|byte| byte == 0) && bytes.iter().all(|byte| *byte == 0)
            }
            (Value::Text(key), Value::Text(text)) => {
                key.bytes().all(|byte| byte == 0) && text.bytes().all(|byte| byte == 0)
            }
            (Value::Text(key), _) => key.bytes().all(|byte| byte == 0),
            _ => false,
        }));

        let result = PairRequestResult {
            device: DeviceId::parse("AAAAAAAAAAAAAAAAAAAAAA").unwrap(),
            host: "Studio Mac".to_owned(),
        };
        let bytes = encode_response(id(1), &result).unwrap();
        let response = decode_response(&bytes);
        assert_eq!(response.decode_result::<PairRequest>().unwrap(), Ok(result));
    }

    #[test]
    fn pair_request_errors() {
        let parse = |method: &str, params: &PairRequestParams| {
            PairRequestParams::parse(&decode_request(
                &encode_request(id(1), method, params).unwrap(),
            ))
        };
        assert!(parse("pair.request", &pair_params(&"é".repeat(64))).is_ok());
        assert_eq!(
            parse("pair.request", &pair_params("")).unwrap_err().code,
            ErrorCode::BadRequest
        );
        assert_eq!(
            parse("pair.request", &pair_params(&"n".repeat(65)))
                .unwrap_err()
                .code,
            ErrorCode::BadRequest
        );
        assert_eq!(
            parse("hello", &pair_params("phone")).unwrap_err().code,
            ErrorCode::UnknownMethod
        );

        // The platform has 1 to 32 characters, and the model has at most 64 (section 8.3).
        let with = |platform: &str, model: Option<String>| {
            let mut params = pair_params("phone");
            params.platform = Platform::from(platform);
            params.model = model;
            parse("pair.request", &params)
        };
        assert!(with(&"p".repeat(32), Some("é".repeat(64))).is_ok());
        assert!(with("harmonyos", None).is_ok());
        for (platform, model) in [
            ("", None),
            (&*"p".repeat(33), None),
            ("ios", Some("m".repeat(65))),
        ] {
            assert_eq!(
                with(platform, model).unwrap_err().code,
                ErrorCode::BadRequest,
                "platform {platform:?}"
            );
        }

        // A token that is not 32 bytes is a bad request.
        let mut params = Value::serialized(&pair_params("phone")).unwrap();
        if let Value::Map(entries) = &mut params {
            for (key, value) in entries.iter_mut() {
                if key.as_text() == Some("token") {
                    *value = Value::Bytes(vec![1; 31]);
                }
            }
        }
        let request = Request {
            id: id(1),
            method: "pair.request".to_owned(),
            params,
        };
        assert_eq!(
            PairRequestParams::parse(&request).unwrap_err().code,
            ErrorCode::BadRequest
        );
    }

    #[test]
    fn results_round_trip() {
        let hello = HelloResult {
            host: HostInfo {
                name: "Studio Mac".to_owned(),
                app: "halv-desktop/0.8.0".to_owned(),
                os: "macos".to_owned(),
            },
            device: DeviceId::parse("AAAAAAAAAAAAAAAAAAAAAA").unwrap(),
            caps: caps::HOST_V1.iter().map(|cap| (*cap).to_owned()).collect(),
            time: 1_760_460_000_000,
        };
        let response = decode_response(&encode_response(id(1), &hello).unwrap());
        assert_eq!(
            response.decode_result::<Hello>().unwrap(),
            Ok(hello.clone())
        );
        assert!(caps::has(&hello.caps, caps::TERMINAL_PASTE_V1));
        assert!(!caps::has(&hello.caps, caps::AGENT_STATUS_V1));

        let list = PaneList {
            panes: vec![sample_pane()],
            rev: 12,
        };
        let response = decode_response(&encode_response(id(2), &list).unwrap());
        assert_eq!(
            response.decode_result::<PanesList>().unwrap(),
            Ok(list.clone())
        );
        assert_eq!(
            response.decode_result::<PanesSubscribe>().unwrap(),
            Ok(list)
        );

        let subscribed = TerminalSubscribeResult {
            sub: 1,
            cols: 80,
            rows: 24,
            seq: 99,
        };
        let response = decode_response(&encode_response(id(3), &subscribed).unwrap());
        assert_eq!(
            response.decode_result::<TerminalSubscribe>().unwrap(),
            Ok(subscribed)
        );

        let response = decode_response(&encode_response(id(4), &Empty {}).unwrap());
        assert_eq!(
            response.decode_result::<DeviceUnpair>().unwrap(),
            Ok(Empty {})
        );

        let response = decode_response(
            &encode_error(id(5), &RpcError::new(ErrorCode::Denied, "denied")).unwrap(),
        );
        assert_eq!(
            response
                .decode_result::<PairRequest>()
                .unwrap()
                .unwrap_err()
                .code,
            ErrorCode::Denied
        );

        // A value of the wrong shape is a protocol error.
        let response = decode_response(&encode_response(id(6), &"text").unwrap());
        assert!(response.decode_result::<Hello>().is_err());
    }

    #[test]
    fn host_events_round_trip() {
        let events = [
            HostEvent::PanesChanged(PaneList {
                panes: vec![sample_pane()],
                rev: 3,
            }),
            HostEvent::TerminalSnapshot(TerminalSnapshot {
                sub: 1,
                seq: 10,
                cols: 80,
                rows: 24,
                data: b"\x1b[2J$ ".to_vec(),
            }),
            HostEvent::TerminalData(TerminalData {
                sub: 1,
                seq: 11,
                data: vec![0, 255, 10],
            }),
            HostEvent::TerminalResized(TerminalResized {
                sub: 1,
                cols: 100,
                rows: 30,
            }),
            HostEvent::TerminalExit(TerminalExit {
                sub: 1,
                code: Some(-1),
                signal: None,
            }),
            HostEvent::TerminalExit(TerminalExit {
                sub: 1,
                code: None,
                signal: Some("SIGTERM".to_owned()),
            }),
            HostEvent::TerminalClosed(TerminalClosed {
                sub: 1,
                reason: "pane closed".to_owned(),
            }),
            HostEvent::DeviceRevoked,
        ];
        for host_event in events {
            let event = decode_event(&host_event.encode().unwrap());
            assert_eq!(event.name, host_event.name());
            assert_eq!(HostEvent::parse(&event).unwrap(), Some(host_event));
        }
    }

    #[test]
    fn device_events_round_trip() {
        let ack = DeviceEvent::TerminalAck(TerminalAck { sub: 2, seq: 77 });
        let event = decode_event(&ack.encode().unwrap());
        assert_eq!(DeviceEvent::parse(&event).unwrap(), Some(ack));
    }

    #[test]
    fn unknown_events_are_ignored() {
        let event = decode_event(&encode_event("panes.sparkle", &Empty {}).unwrap());
        assert_eq!(HostEvent::parse(&event).unwrap(), None);
        assert_eq!(DeviceEvent::parse(&event).unwrap(), None);
        // A known event with a bad payload fails.
        let event = decode_event(&encode_event("terminal.data", &Empty {}).unwrap());
        assert!(HostEvent::parse(&event).is_err());
        // `device.revoked` has no payload, so any payload gives the event.
        let event = decode_event(&encode_event("device.revoked", &"unexpected").unwrap());
        assert_eq!(
            HostEvent::parse(&event).unwrap(),
            Some(HostEvent::DeviceRevoked)
        );
    }

    #[test]
    fn unknown_text_values_are_kept() {
        let mut pane = sample_pane();
        pane.kind = PaneKind::from("aider");
        pane.status.as_mut().unwrap().state = AgentState::from("thinking");
        pane.status.as_mut().unwrap().prompt = Some(Prompt::Other {
            kind: "form".to_owned(),
        });
        let list = PaneList {
            panes: vec![pane.clone()],
            rev: 1,
        };
        let event = decode_event(&HostEvent::PanesChanged(list).encode().unwrap());
        let Some(HostEvent::PanesChanged(decoded)) = HostEvent::parse(&event).unwrap() else {
            panic!("not panes.changed");
        };
        assert_eq!(decoded.panes[0], pane);
        assert_eq!(decoded.panes[0].kind.as_str(), "aider");

        assert_eq!(Platform::from("ios"), Platform::Ios);
        assert_eq!(
            Platform::from("visionos".to_owned()),
            Platform::Other("visionos".to_owned())
        );
        assert_eq!(PaneKind::from("shell".to_owned()), PaneKind::Shell);
        assert_eq!(ErrorCode::from("rate_limited").as_str(), "rate_limited");
        assert_eq!(AgentState::NeedsInput.to_string(), "needsInput");

        let hello = HelloParams {
            app: "x".to_owned(),
            platform: Platform::from("harmonyos"),
            caps: vec!["future.v9".to_owned()],
        };
        let request = decode_request(&encode_call::<Hello>(id(1), &hello).unwrap());
        assert_eq!(
            SessionRequest::parse(&request).unwrap(),
            SessionRequest::Hello(hello)
        );
    }

    #[test]
    fn closed_values_and_required_fields_are_checked() {
        let mut pane = Value::serialized(&sample_pane()).unwrap();
        if let Value::Map(entries) = &mut pane {
            for (key, value) in entries.iter_mut() {
                if key.as_text() == Some("state") {
                    *value = text("paused");
                }
            }
        }
        assert!(pane.deserialized::<Pane>().is_err());

        let approval_without_tool = map(vec![("kind", text("approval"))]);
        assert!(approval_without_tool.deserialized::<Prompt>().is_err());
        let question = map(vec![
            ("kind", text("question")),
            ("text", text("Continue?")),
            ("options", Value::Array(vec![text("yes"), text("no")])),
            ("extra", Value::Bool(true)),
        ]);
        assert_eq!(
            question.deserialized::<Prompt>().unwrap(),
            Prompt::Question {
                text: "Continue?".to_owned(),
                options: Some(vec!["yes".to_owned(), "no".to_owned()]),
            }
        );
    }

    #[test]
    fn missing_prompt_is_null_on_the_wire() {
        let status = AgentStatus {
            state: AgentState::Idle,
            reason: None,
            updated_at: 1,
            unread: false,
            prompt: None,
        };
        let value = Value::serialized(&status).unwrap();
        assert!(value
            .as_map()
            .unwrap()
            .iter()
            .any(|(key, value)| key.as_text() == Some("prompt") && value.is_null()));
        let without = map(vec![
            ("state", text("idle")),
            ("updatedAt", Value::Integer(1.into())),
            ("unread", Value::Bool(false)),
        ]);
        assert_eq!(without.deserialized::<AgentStatus>().unwrap(), status);
    }

    #[test]
    fn error_codes() {
        let all = [
            (ErrorCode::BadRequest, "bad_request"),
            (ErrorCode::UnknownMethod, "unknown_method"),
            (ErrorCode::NotFound, "not_found"),
            (ErrorCode::Denied, "denied"),
            (ErrorCode::Timeout, "timeout"),
            (ErrorCode::Busy, "busy"),
            (ErrorCode::Expired, "expired"),
            (ErrorCode::Unavailable, "unavailable"),
            (ErrorCode::Incompatible, "incompatible"),
            (ErrorCode::TooLarge, "too_large"),
            (ErrorCode::Internal, "internal"),
        ];
        for (code, text) in all {
            assert_eq!(code.as_str(), text);
            assert_eq!(ErrorCode::from(text), code);
        }
        let error = RpcError::new(ErrorCode::Expired, "the offer ended");
        assert_eq!(error.to_string(), "expired: the offer ended");
        let response = decode_response(&encode_error(id(1), &error).unwrap());
        assert_eq!(response.outcome, Err(error));
    }

    #[test]
    fn known_text_values_of_the_contract() {
        for (state, text) in [
            (AgentState::Working, "working"),
            (AgentState::Done, "done"),
            (AgentState::NeedsInput, "needsInput"),
            (AgentState::NeedsApproval, "needsApproval"),
            (AgentState::Idle, "idle"),
            (AgentState::Exited, "exited"),
            (AgentState::Failed, "failed"),
            (AgentState::Interrupted, "interrupted"),
        ] {
            assert_eq!(AgentState::from(text), state);
            assert_eq!(state.as_str(), text);
        }
        for (kind, text) in [
            (PaneKind::Claude, "claude"),
            (PaneKind::Codex, "codex"),
            (PaneKind::Kimi, "kimi"),
            (PaneKind::Glm, "glm"),
            (PaneKind::Grok, "grok"),
            (PaneKind::Gemini, "gemini"),
            (PaneKind::Shell, "shell"),
        ] {
            assert_eq!(PaneKind::from(text), kind);
            assert_eq!(kind.as_str(), text);
        }
        assert_eq!(Platform::from("android"), Platform::Android);
        assert_eq!(Platform::Ios.as_str(), "ios");
    }

    #[test]
    fn debug_output_has_no_content() {
        let pair = encode_call::<PairRequest>(id(1), &pair_params("phone")).unwrap();
        let message = Message::decode(&pair).unwrap();
        let params = PairRequestParams::parse(match &message {
            Message::Request(request) => request,
            _ => unreachable!(),
        })
        .unwrap();
        let input = TerminalInputParams {
            session: "s".to_owned(),
            data: b"hunter2".to_vec(),
        };
        let paste = TerminalPasteParams {
            session: "s".to_owned(),
            text: "secret paste".to_owned(),
            submit: false,
        };
        let data = TerminalData {
            sub: 1,
            seq: 1,
            data: b"password: hunter2".to_vec(),
        };
        let debug = format!("{message:?} {params:?} {input:?} {paste:?} {data:?}");
        assert!(!debug.contains("90, 90"));
        assert!(!debug.contains("hunter2"));
        assert!(!debug.contains("104, 117"));
        assert!(!debug.contains("secret paste"));
        assert!(debug.contains("RelayToken(<redacted>)"));
        assert!(debug.contains("data_len: 7"));
    }
}
