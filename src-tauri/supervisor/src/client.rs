use crate::{
    negotiated_protocol, Event, OpenRequest, RequestFrame, ResponseFrame, SessionSummary, Snapshot,
    MAX_PROTOCOL_VERSION, PROTOCOL_VERSION,
};
use std::{
    io::{BufRead, BufReader, Write},
    mem,
    os::{
        fd::{FromRawFd, IntoRawFd, OwnedFd},
        unix::{ffi::OsStrExt, net::UnixStream},
    },
    path::Path,
    time::Duration,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

pub struct SupervisorClient {
    reader: BufReader<UnixStream>,
    supervisor_pid: u32,
    supervisor_version: String,
    supervisor_fingerprint: Option<String>,
    protocol: u32,
}

struct Hello {
    supervisor_pid: u32,
    supervisor_version: String,
    supervisor_fingerprint: Option<String>,
    protocol: u32,
}

impl SupervisorClient {
    pub fn connect(socket_path: impl AsRef<Path>, app_version: &str) -> Result<Self, String> {
        let stream = connect_with_timeout(socket_path.as_ref(), REQUEST_TIMEOUT)?;
        Self::from_stream(stream, app_version)
    }

    pub(crate) fn from_stream(stream: UnixStream, app_version: &str) -> Result<Self, String> {
        let mut reader = stream_reader(stream)?;
        let hello = perform_hello(&mut reader, app_version)?;
        Ok(Self {
            reader,
            supervisor_pid: hello.supervisor_pid,
            supervisor_version: hello.supervisor_version,
            supervisor_fingerprint: hello.supervisor_fingerprint,
            protocol: hello.protocol,
        })
    }

    pub fn supervisor_pid(&self) -> u32 {
        self.supervisor_pid
    }

    pub fn supervisor_version(&self) -> &str {
        &self.supervisor_version
    }

    pub fn supervisor_fingerprint(&self) -> Option<&str> {
        self.supervisor_fingerprint.as_deref()
    }

    /// Returns the protocol revision that the hello negotiated.
    pub fn negotiated_protocol(&self) -> u32 {
        self.protocol
    }

    pub fn open(&mut self, request: OpenRequest) -> Result<SessionSummary, String> {
        match self.request(RequestFrame::from(request))? {
            ResponseFrame::Session { session } => Ok(session),
            frame => Err(unexpected_response("open terminal", &frame)),
        }
    }

    pub fn write(&mut self, id: &str, data: &[u8]) -> Result<(), String> {
        let request = RequestFrame::Write {
            id: id.to_string(),
            data: encode_base64(data),
        };
        expect_ok(self.request(request)?, "write terminal input")
    }

    pub fn resize(&mut self, id: &str, cols: u16, rows: u16) -> Result<SessionSummary, String> {
        let request = RequestFrame::Resize {
            id: id.to_string(),
            cols,
            rows,
        };
        match self.request(request)? {
            ResponseFrame::Session { session } => Ok(session),
            frame => Err(unexpected_response("resize terminal", &frame)),
        }
    }

    pub fn close(&mut self, id: &str) -> Result<SessionSummary, String> {
        let request = RequestFrame::Close { id: id.to_string() };
        match self.request(request)? {
            ResponseFrame::Session { session } => Ok(session),
            frame => Err(unexpected_response("close terminal", &frame)),
        }
    }

    pub fn remove(&mut self, id: &str) -> Result<(), String> {
        expect_ok(
            self.request(RequestFrame::Remove { id: id.to_string() })?,
            "remove terminal",
        )
    }

    pub fn list(&mut self) -> Result<Vec<SessionSummary>, String> {
        match self.request(RequestFrame::List)? {
            ResponseFrame::List { sessions } => Ok(sessions),
            frame => Err(unexpected_response("list terminals", &frame)),
        }
    }

    pub fn update_meta(
        &mut self,
        id: &str,
        meta: serde_json::Value,
    ) -> Result<SessionSummary, String> {
        let request = RequestFrame::UpdateMeta {
            id: id.to_string(),
            meta,
        };
        match self.request(request)? {
            ResponseFrame::Session { session } => Ok(session),
            frame => Err(unexpected_response("update terminal metadata", &frame)),
        }
    }

    pub fn shutdown(&mut self, kill_sessions: bool) -> Result<(), String> {
        expect_ok(
            self.request(RequestFrame::Shutdown { kill_sessions })?,
            "shut down supervisor",
        )
    }

    pub fn attach(
        socket_path: impl AsRef<Path>,
        app_version: &str,
        id: &str,
    ) -> Result<(Snapshot, AttachEvents), String> {
        Self::attach_from_sequence(socket_path, app_version, id, None)
    }

    pub fn attach_from_sequence(
        socket_path: impl AsRef<Path>,
        app_version: &str,
        id: &str,
        from_sequence: Option<u64>,
    ) -> Result<(Snapshot, AttachEvents), String> {
        let stream = connect_with_timeout(socket_path.as_ref(), REQUEST_TIMEOUT)?;
        Self::attach_stream(
            stream,
            app_version,
            id,
            resume_protocol_cursor(from_sequence),
        )
    }

    pub(crate) fn attach_stream(
        stream: UnixStream,
        app_version: &str,
        id: &str,
        from_sequence: Option<u64>,
    ) -> Result<(Snapshot, AttachEvents), String> {
        let mut reader = stream_reader(stream)?;
        perform_hello(&mut reader, app_version)?;
        write_frame(
            reader.get_mut(),
            &RequestFrame::Attach {
                id: id.to_string(),
                from_sequence,
            },
        )?;
        let frame = read_response(&mut reader)?;
        let snapshot = match frame {
            ResponseFrame::Snapshot {
                session,
                sequence,
                scrollback,
            } => Snapshot {
                session,
                sequence,
                scrollback: decode_utf8(&scrollback, "terminal snapshot")?,
            },
            ResponseFrame::Error { message } => return Err(message),
            other => return Err(unexpected_response("attach terminal", &other)),
        };
        reader
            .get_mut()
            .set_read_timeout(None)
            .map_err(|error| format!("failed to clear terminal attach timeout: {error}"))?;
        Ok((snapshot, AttachEvents { reader }))
    }

    fn request(&mut self, request: RequestFrame) -> Result<ResponseFrame, String> {
        // An older supervisor closes the connection on an unknown request.
        check_request_protocol(&request, self.protocol)?;
        write_frame(self.reader.get_mut(), &request)?;
        let response = read_response(&mut self.reader)?;
        match response {
            ResponseFrame::Error { message } => Err(message),
            other => Ok(other),
        }
    }
}

fn check_request_protocol(request: &RequestFrame, protocol: u32) -> Result<(), String> {
    if request.min_protocol() > protocol {
        // Keep this text apart from the broken-connection and protocol
        // mismatch texts, so the app keeps the connection.
        return Err(format!(
            "the terminal supervisor does not support this request (revision {protocol})"
        ));
    }
    Ok(())
}

pub(crate) fn resume_protocol_cursor(from_sequence: Option<u64>) -> Option<u64> {
    from_sequence.map(|sequence| sequence.saturating_sub(1))
}

pub struct AttachEvents {
    reader: BufReader<UnixStream>,
}

impl Iterator for AttachEvents {
    type Item = Result<Event, String>;

    fn next(&mut self) -> Option<Self::Item> {
        match read_response_optional(&mut self.reader) {
            Ok(Some(ResponseFrame::Data { sequence, data })) => {
                Some(decode_utf8(&data, "terminal data").map(|data| Event::Data { sequence, data }))
            }
            Ok(Some(ResponseFrame::Exit {
                sequence,
                exit_code,
                signal,
            })) => Some(Ok(Event::Exit {
                sequence,
                exit_code,
                signal,
            })),
            Ok(Some(ResponseFrame::Error { message })) => Some(Err(message)),
            Ok(Some(frame)) => Some(Err(unexpected_response("read terminal event", &frame))),
            Ok(None) => None,
            Err(error) => Some(Err(error)),
        }
    }
}

fn stream_reader(stream: UnixStream) -> Result<BufReader<UnixStream>, String> {
    stream
        .set_read_timeout(Some(REQUEST_TIMEOUT))
        .map_err(|error| format!("failed to set supervisor read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(REQUEST_TIMEOUT))
        .map_err(|error| format!("failed to set supervisor write timeout: {error}"))?;
    Ok(BufReader::new(stream))
}

fn perform_hello(reader: &mut BufReader<UnixStream>, app_version: &str) -> Result<Hello, String> {
    write_frame(
        reader.get_mut(),
        &RequestFrame::Hello {
            proto: PROTOCOL_VERSION,
            app_version: app_version.to_string(),
            max_proto: Some(MAX_PROTOCOL_VERSION),
        },
    )?;
    match read_response(reader)? {
        ResponseFrame::Hello {
            proto,
            supervisor_version,
            supervisor_fingerprint,
            pid,
            max_proto,
        } if proto == PROTOCOL_VERSION => Ok(Hello {
            supervisor_pid: pid,
            supervisor_version,
            supervisor_fingerprint,
            protocol: negotiated_protocol(MAX_PROTOCOL_VERSION, max_proto),
        }),
        ResponseFrame::Hello { proto, .. } => Err(format!(
            "failed to connect to supervisor: protocol {proto} is not supported"
        )),
        ResponseFrame::Error { message } => Err(message),
        frame => Err(unexpected_response("connect to supervisor", &frame)),
    }
}

fn expect_ok(frame: ResponseFrame, operation: &str) -> Result<(), String> {
    match frame {
        ResponseFrame::Ok => Ok(()),
        other => Err(unexpected_response(operation, &other)),
    }
}

fn unexpected_response(operation: &str, frame: &ResponseFrame) -> String {
    format!("failed to {operation}: unexpected supervisor response {frame:?}")
}

fn write_frame(writer: &mut UnixStream, frame: &RequestFrame) -> Result<(), String> {
    serde_json::to_writer(&mut *writer, frame)
        .map_err(|error| format!("failed to encode supervisor request: {error}"))?;
    writer
        .write_all(b"\n")
        .and_then(|()| writer.flush())
        .map_err(|error| format!("failed to send supervisor request: {error}"))
}

fn read_response(reader: &mut BufReader<UnixStream>) -> Result<ResponseFrame, String> {
    read_response_optional(reader)?
        .ok_or_else(|| "failed to read supervisor response: connection closed".to_string())
}

fn read_response_optional(
    reader: &mut BufReader<UnixStream>,
) -> Result<Option<ResponseFrame>, String> {
    let mut line = String::new();
    let read = reader
        .read_line(&mut line)
        .map_err(|error| format!("failed to read supervisor response: {error}"))?;
    if read == 0 {
        return Ok(None);
    }
    serde_json::from_str(&line)
        .map(Some)
        .map_err(|error| format!("failed to decode supervisor response: {error}"))
}

fn connect_with_timeout(path: &Path, timeout: Duration) -> Result<UnixStream, String> {
    let path_bytes = path.as_os_str().as_bytes();
    let address_offset = mem::offset_of!(libc::sockaddr_un, sun_path);
    let max_path = mem::size_of::<libc::sockaddr_un>() - address_offset;
    if path_bytes.is_empty() || path_bytes.len() + 1 > max_path {
        return Err(format!(
            "failed to connect to supervisor socket `{}`: path is too long",
            path.display()
        ));
    }

    let raw_fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if raw_fd == -1 {
        return Err(format!(
            "failed to create supervisor socket: {}",
            std::io::Error::last_os_error()
        ));
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
    let flags = unsafe { libc::fcntl(raw_fd, libc::F_GETFL) };
    if flags == -1 || unsafe { libc::fcntl(raw_fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1
    {
        return Err(format!(
            "failed to configure supervisor socket: {}",
            std::io::Error::last_os_error()
        ));
    }

    let mut address = unsafe { mem::zeroed::<libc::sockaddr_un>() };
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (destination, source) in address.sun_path.iter_mut().zip(path_bytes) {
        *destination = *source as libc::c_char;
    }
    let address_len = (address_offset + path_bytes.len() + 1) as libc::socklen_t;
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    {
        address.sun_len = address_len as u8;
    }

    let connected = unsafe {
        libc::connect(
            raw_fd,
            (&raw const address).cast::<libc::sockaddr>(),
            address_len,
        )
    };
    if connected == -1 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(format!(
                "failed to connect to supervisor socket `{}`: {error}",
                path.display()
            ));
        }
        let mut poll_fd = libc::pollfd {
            fd: raw_fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;
        let ready = unsafe { libc::poll(&mut poll_fd, 1, timeout_ms) };
        if ready == 0 {
            return Err(format!(
                "failed to connect to supervisor socket `{}`: timed out",
                path.display()
            ));
        }
        if ready == -1 {
            return Err(format!(
                "failed to connect to supervisor socket `{}`: {}",
                path.display(),
                std::io::Error::last_os_error()
            ));
        }
        let mut socket_error = 0_i32;
        let mut socket_error_len = mem::size_of::<i32>() as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                raw_fd,
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&raw mut socket_error).cast(),
                &mut socket_error_len,
            )
        } == -1
        {
            return Err(format!(
                "failed to inspect supervisor connection: {}",
                std::io::Error::last_os_error()
            ));
        }
        if socket_error != 0 {
            return Err(format!(
                "failed to connect to supervisor socket `{}`: {}",
                path.display(),
                std::io::Error::from_raw_os_error(socket_error)
            ));
        }
    }

    if unsafe { libc::fcntl(raw_fd, libc::F_SETFL, flags) } == -1 {
        return Err(format!(
            "failed to restore supervisor socket mode: {}",
            std::io::Error::last_os_error()
        ));
    }
    let stream = unsafe { UnixStream::from_raw_fd(fd.into_raw_fd()) };
    Ok(stream)
}

pub(crate) fn encode_base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        encoded.push(TABLE[(first >> 2) as usize] as char);
        encoded.push(TABLE[(((first & 0x03) << 4) | (second >> 4)) as usize] as char);
        encoded.push(if chunk.len() > 1 {
            TABLE[(((second & 0x0f) << 2) | (third >> 6)) as usize] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            TABLE[(third & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    encoded
}

pub(crate) fn decode_base64(data: &str) -> Result<Vec<u8>, String> {
    if !data.len().is_multiple_of(4) {
        return Err("base64 length is not a multiple of four".to_string());
    }
    let bytes = data.as_bytes();
    let mut decoded = Vec::with_capacity(data.len() / 4 * 3);
    for (index, chunk) in bytes.chunks(4).enumerate() {
        let last = index + 1 == bytes.len() / 4;
        let a = base64_value(chunk[0])?;
        let b = base64_value(chunk[1])?;
        let c = if chunk[2] == b'=' {
            if !last || chunk[3] != b'=' {
                return Err("base64 padding is invalid".to_string());
            }
            0
        } else {
            base64_value(chunk[2])?
        };
        let d = if chunk[3] == b'=' {
            if !last {
                return Err("base64 padding is invalid".to_string());
            }
            0
        } else {
            base64_value(chunk[3])?
        };
        decoded.push((a << 2) | (b >> 4));
        if chunk[2] != b'=' {
            decoded.push((b << 4) | (c >> 2));
        }
        if chunk[3] != b'=' {
            decoded.push((c << 6) | d);
        }
    }
    Ok(decoded)
}

fn base64_value(byte: u8) -> Result<u8, String> {
    match byte {
        b'A'..=b'Z' => Ok(byte - b'A'),
        b'a'..=b'z' => Ok(byte - b'a' + 26),
        b'0'..=b'9' => Ok(byte - b'0' + 52),
        b'+' => Ok(62),
        b'/' => Ok(63),
        _ => Err("base64 data contains an invalid character".to_string()),
    }
}

fn decode_utf8(data: &str, label: &str) -> Result<String, String> {
    let decoded =
        decode_base64(data).map_err(|error| format!("failed to decode {label}: {error}"))?;
    String::from_utf8(decoded).map_err(|error| format!("failed to decode {label}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_reports_supervisor_version_and_fingerprint() {
        let (client_stream, mut server_stream) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let mut reader = BufReader::new(server_stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            match serde_json::from_str::<RequestFrame>(&line).unwrap() {
                RequestFrame::Hello {
                    proto,
                    app_version,
                    max_proto,
                } => {
                    assert_eq!(proto, PROTOCOL_VERSION);
                    assert_eq!(app_version, "0.4.15");
                    assert_eq!(max_proto, Some(MAX_PROTOCOL_VERSION));
                }
                request => panic!("expected hello request, got {request:?}"),
            }
            serde_json::to_writer(
                &mut server_stream,
                &ResponseFrame::Hello {
                    proto: PROTOCOL_VERSION,
                    supervisor_version: "0.4.14".to_string(),
                    supervisor_fingerprint: Some("0123456789abcdef".to_string()),
                    pid: 42,
                    max_proto: Some(MAX_PROTOCOL_VERSION),
                },
            )
            .unwrap();
            server_stream.write_all(b"\n").unwrap();
            server_stream.flush().unwrap();
        });

        let client = SupervisorClient::from_stream(client_stream, "0.4.15").unwrap();
        assert_eq!(client.supervisor_pid(), 42);
        assert_eq!(client.supervisor_version(), "0.4.14");
        assert_eq!(client.supervisor_fingerprint(), Some("0123456789abcdef"));
        server.join().unwrap();
    }

    #[test]
    fn hello_without_supervisor_fingerprint_still_connects() {
        let (client_stream, mut server_stream) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let mut reader = BufReader::new(server_stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            serde_json::from_str::<RequestFrame>(&line).unwrap();
            server_stream
                .write_all(
                    br#"{"type":"hello","proto":1,"supervisor_version":"0.4.14","pid":42}
"#,
                )
                .unwrap();
            server_stream.flush().unwrap();
        });

        let client = SupervisorClient::from_stream(client_stream, "0.4.15").unwrap();
        assert_eq!(client.supervisor_version(), "0.4.14");
        assert_eq!(client.supervisor_fingerprint(), None);
        assert_eq!(client.negotiated_protocol(), PROTOCOL_VERSION);
        server.join().unwrap();
    }

    #[test]
    fn hello_fingerprint_is_ignored_by_legacy_clients() {
        #[derive(serde::Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        enum LegacyResponseFrame {
            Hello {
                proto: u32,
                supervisor_version: String,
                pid: u32,
            },
        }

        let response = serde_json::to_string(&ResponseFrame::Hello {
            proto: PROTOCOL_VERSION,
            supervisor_version: "0.4.15".to_string(),
            supervisor_fingerprint: Some("0123456789abcdef".to_string()),
            pid: 42,
            max_proto: Some(MAX_PROTOCOL_VERSION),
        })
        .unwrap();
        let LegacyResponseFrame::Hello {
            proto,
            supervisor_version,
            pid,
        } = serde_json::from_str(&response).unwrap();
        assert_eq!(proto, PROTOCOL_VERSION);
        assert_eq!(supervisor_version, "0.4.15");
        assert_eq!(pid, 42);
    }

    #[test]
    fn a_0_6_3_supervisor_hello_negotiates_protocol_1() {
        // The supervisor that Halv 0.6.3 started still answers like this.
        let (client_stream, mut server_stream) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let mut reader = BufReader::new(server_stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            server_stream
                .write_all(
                    br#"{"type":"hello","proto":1,"supervisor_version":"0.6.3","supervisor_fingerprint":"0123456789abcdef","pid":1125}
{"type":"list","sessions":[]}
"#,
                )
                .unwrap();
            server_stream.flush().unwrap();
            line.clear();
            reader.read_line(&mut line).unwrap();
            line
        });

        let mut client = SupervisorClient::from_stream(client_stream, "0.7.2").unwrap();
        assert_eq!(client.supervisor_pid(), 1125);
        assert_eq!(client.negotiated_protocol(), PROTOCOL_VERSION);
        assert!(client.list().unwrap().is_empty());
        assert_eq!(server.join().unwrap(), "{\"type\":\"list\"}\n");
    }

    #[test]
    fn requests_above_the_negotiated_protocol_stay_local() {
        let list = RequestFrame::List;
        assert_eq!(check_request_protocol(&list, PROTOCOL_VERSION), Ok(()));
        let error = check_request_protocol(&list, 0)
            .expect_err("a request above the negotiated revision must not be sent");
        assert!(error.contains("revision 0"), "{error}");
        assert!(!error.contains("protocol"), "{error}");
    }

    #[test]
    fn base64_round_trip_covers_all_tail_lengths() {
        for input in [b"".as_slice(), b"a", b"ab", b"abc", b"abcdefg"] {
            assert_eq!(decode_base64(&encode_base64(input)).unwrap(), input);
        }
    }

    #[test]
    fn base64_matches_standard_examples() {
        assert_eq!(encode_base64(b"f"), "Zg==");
        assert_eq!(encode_base64(b"fo"), "Zm8=");
        assert_eq!(encode_base64(b"foo"), "Zm9v");
    }

    #[test]
    fn resume_sequence_is_inclusive_for_callers() {
        assert_eq!(resume_protocol_cursor(None), None);
        assert_eq!(resume_protocol_cursor(Some(1)), Some(0));
        assert_eq!(resume_protocol_cursor(Some(8)), Some(7));
    }
}
