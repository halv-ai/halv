//! Conformance item 7 of section 13 against the mock relay: host signature checks,
//! invite tries and expiry, device admission, `devices_sync`, forwarding, close codes,
//! and frame size limits. The tests speak the relay protocol with raw sockets.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use halv_remote::relay_auth::{bearer_header_value, HostAuthHeaders};
use halv_remote::relay_wire::{device_socket_url, host_socket_url, MAX_DEVICE_FRAME};
use halv_remote::testing::{MockRelay, MockRelayConfig};
use halv_remote::{unix_time_ms, HostIdentity, RelayToken, TokenHash};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderName;
use tokio_tungstenite::tungstenite::{self, Message};

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

const WAIT: Duration = Duration::from_secs(5);
const DEVICE_A: &str = "AAAAAAAAAAAAAAAAAAAAAA";
const DEVICE_B: &str = "BBBBBBBBBBBBBBBBBBBBBB";
const INVITE: &str = "IIIIIIIIIIIIIIIIIIIIII";

/// The HTTP status and the `code` of a refused upgrade.
fn refusal(error: tungstenite::Error) -> (u16, Option<String>, Option<String>) {
    match error {
        tungstenite::Error::Http(response) => {
            let code = response
                .body()
                .as_deref()
                .and_then(|body| serde_json::from_slice::<Value>(body).ok())
                .and_then(|body| body.get("code").and_then(Value::as_str).map(str::to_owned));
            let retry = response
                .headers()
                .get("Retry-After")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            (response.status().as_u16(), code, retry)
        }
        other => panic!("expected an HTTP refusal, got {other}"),
    }
}

// The tests match on the tungstenite error. Boxing it only adds noise.
#[allow(clippy::result_large_err)]
async fn host_socket_at(
    relay: &MockRelay,
    identity: &HostIdentity,
    path_identity: &HostIdentity,
    timestamp: u64,
) -> Result<Socket, tungstenite::Error> {
    let url = host_socket_url(&relay.url(), &path_identity.host_id());
    let mut request = url.into_client_request()?;
    let headers = HostAuthHeaders::sign(identity, timestamp);
    for (name, value) in headers.pairs() {
        request.headers_mut().insert(
            HeaderName::from_bytes(name.as_bytes()).unwrap(),
            value.parse().unwrap(),
        );
    }
    tokio_tungstenite::connect_async(request)
        .await
        .map(|(socket, _)| socket)
}

/// Opens an authenticated host socket and reads `ready`.
async fn host_socket(relay: &MockRelay, identity: &HostIdentity, timestamp: u64) -> Socket {
    let mut socket = host_socket_at(relay, identity, identity, timestamp)
        .await
        .unwrap();
    assert_eq!(next_json(&mut socket).await, json!({"t": "ready", "v": 1}));
    socket
}

// The tests match on the tungstenite error. Boxing it only adds noise.
#[allow(clippy::result_large_err)]
async fn device_socket(
    relay: &MockRelay,
    identity: &HostIdentity,
    authorization: Option<&str>,
) -> Result<Socket, tungstenite::Error> {
    let url = device_socket_url(&relay.url(), &identity.host_id());
    let mut request = url.into_client_request()?;
    if let Some(value) = authorization {
        request
            .headers_mut()
            .insert("Authorization", value.parse().unwrap());
    }
    tokio_tungstenite::connect_async(request)
        .await
        .map(|(socket, _)| socket)
}

fn bearer(token: &[u8; 32]) -> String {
    bearer_header_value(token).as_str().to_owned()
}

fn hash(token: &[u8; 32]) -> String {
    TokenHash::of(token).as_str().to_owned()
}

async fn next_message(socket: &mut Socket) -> Message {
    loop {
        let message = tokio::time::timeout(WAIT, socket.next())
            .await
            .expect("a frame in time")
            .expect("an open socket")
            .expect("a valid frame");
        if !matches!(message, Message::Ping(_) | Message::Pong(_)) {
            return message;
        }
    }
}

async fn next_json(socket: &mut Socket) -> Value {
    match next_message(socket).await {
        Message::Text(text) => serde_json::from_str(text.as_str()).unwrap(),
        other => panic!("expected a text frame, got {other:?}"),
    }
}

async fn send_json(socket: &mut Socket, value: Value) {
    socket.send(Message::text(value.to_string())).await.unwrap();
}

/// Sends a control request and returns the answer.
async fn control(socket: &mut Socket, value: Value) -> Value {
    send_json(socket, value).await;
    next_json(socket).await
}

async fn close_code(socket: &mut Socket) -> Option<u16> {
    loop {
        match tokio::time::timeout(WAIT, socket.next()).await.unwrap() {
            Some(Ok(Message::Close(frame))) => return frame.map(|frame| frame.code.into()),
            Some(Ok(_)) => continue,
            Some(Err(_)) | None => return None,
        }
    }
}

async fn http_get(relay: &MockRelay, path: &str) -> String {
    let mut stream = tokio::net::TcpStream::connect(relay.addr()).await.unwrap();
    let request = format!("GET {path} HTTP/1.1\r\nHost: relay\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    response
}

#[tokio::test]
async fn health_and_routing() {
    let relay = MockRelay::start().await.unwrap();
    let health = http_get(&relay, "/health").await;
    assert!(health.starts_with("HTTP/1.1 200"));
    assert!(health.ends_with("\r\n\r\nok"));
    let unknown = http_get(&relay, "/v2/anything").await;
    assert!(unknown.starts_with("HTTP/1.1 404"));
    assert!(unknown.ends_with("{\"code\":\"not_found\"}"));
    let bad_id = http_get(&relay, "/v1/hosts/short/host").await;
    assert!(bad_id.starts_with("HTTP/1.1 404"));
    let no_upgrade = http_get(&relay, "/v1/hosts/AAAAAAAAAAAAAAAAAAAAAA/device").await;
    assert!(no_upgrade.starts_with("HTTP/1.1 426"));
    assert!(no_upgrade.ends_with("{\"code\":\"upgrade_required\"}"));
}

#[tokio::test]
async fn host_signature_checks() {
    let relay = MockRelay::start().await.unwrap();
    let identity = HostIdentity::generate().unwrap();
    let other = HostIdentity::generate().unwrap();
    let now = unix_time_ms();

    // Check 1: missing headers.
    let url = host_socket_url(&relay.url(), &identity.host_id());
    let error = tokio_tungstenite::connect_async(url.as_str())
        .await
        .unwrap_err();
    assert_eq!(refusal(error).0, 400);
    // Check 2: the key of another host.
    let error = host_socket_at(&relay, &other, &identity, now)
        .await
        .unwrap_err();
    assert_eq!(refusal(error).0, 401);
    // Check 4: clock skew in both directions.
    let error = host_socket_at(&relay, &identity, &identity, now - 400_000)
        .await
        .unwrap_err();
    assert_eq!(refusal(error).1.as_deref(), Some("clock_skew"));
    let error = host_socket_at(&relay, &identity, &identity, now + 400_000)
        .await
        .unwrap_err();
    assert_eq!(refusal(error).1.as_deref(), Some("clock_skew"));

    // A valid host gets ready, and the relay stores the timestamp.
    let mut first = host_socket(&relay, &identity, now).await;
    assert_eq!(relay.last_auth_ts(&identity.host_id()), Some(now));
    // Check 5: the same timestamp again is a replay.
    let error = host_socket_at(&relay, &identity, &identity, now)
        .await
        .unwrap_err();
    assert_eq!(refusal(error).1.as_deref(), Some("replay"));
    // A newer socket replaces the first one with 4409.
    let _second = host_socket(&relay, &identity, now + 1).await;
    assert_eq!(close_code(&mut first).await, Some(4409));
    assert!(relay.host_connected(&identity.host_id()));
}

#[tokio::test]
async fn bad_signature_is_refused() {
    let relay = MockRelay::start().await.unwrap();
    let identity = HostIdentity::generate().unwrap();
    let headers = HostAuthHeaders::sign(&identity, unix_time_ms());
    let mut signature = halv_remote::b64u::decode(&headers.signature).unwrap();
    signature[10] ^= 1;
    let url = host_socket_url(&relay.url(), &identity.host_id());
    let mut request = url.into_client_request().unwrap();
    request
        .headers_mut()
        .insert("X-Halv-Host-Key", headers.host_key.parse().unwrap());
    request
        .headers_mut()
        .insert("X-Halv-Timestamp", headers.timestamp.parse().unwrap());
    request.headers_mut().insert(
        "X-Halv-Signature",
        halv_remote::b64u::encode(&signature).parse().unwrap(),
    );
    let error = tokio_tungstenite::connect_async(request).await.unwrap_err();
    assert_eq!(refusal(error), (401, Some("unauthorized".to_owned()), None));
}

#[tokio::test]
async fn device_admission_with_invites() {
    let relay = MockRelay::start().await.unwrap();
    let identity = HostIdentity::generate().unwrap();
    let invite_token = [0x31u8; 32];

    // No host socket: 503 host_offline with Retry-After 5.
    let error = device_socket(&relay, &identity, Some(&bearer(&invite_token)))
        .await
        .unwrap_err();
    assert_eq!(
        refusal(error),
        (503, Some("host_offline".to_owned()), Some("5".to_owned()))
    );

    let mut host = host_socket(&relay, &identity, unix_time_ms()).await;
    // A missing token, a malformed token, and an unknown token: 401.
    for authorization in [
        None,
        Some("Bearer short"),
        Some(bearer(&[9u8; 32]).as_str()),
    ] {
        let error = device_socket(&relay, &identity, authorization)
            .await
            .unwrap_err();
        assert_eq!(refusal(error).0, 401);
    }

    // Invite rules: tries from 1 to 5, an expiry in the future.
    let exp = relay.now_ms() + 60_000;
    let answer = control(
        &mut host,
        json!({"t": "invite_put", "rid": 1, "invite": INVITE, "hash": hash(&invite_token), "exp": exp, "tries": 6}),
    )
    .await;
    assert_eq!(answer["t"], "err");
    assert_eq!(answer["rid"], 1);
    let answer = control(
        &mut host,
        json!({"t": "invite_put", "rid": 2, "invite": INVITE, "hash": hash(&invite_token), "exp": relay.now_ms() - 1, "tries": 2}),
    )
    .await;
    assert_eq!(answer["t"], "err");
    let answer = control(
        &mut host,
        json!({"t": "invite_put", "rid": 3, "invite": INVITE, "hash": hash(&invite_token), "exp": exp, "tries": 2}),
    )
    .await;
    assert_eq!(answer, json!({"t": "ok", "rid": 3}));

    // Two tries admit two sockets. Each admission sends conn_open.
    let _first = device_socket(&relay, &identity, Some(&bearer(&invite_token)))
        .await
        .unwrap();
    let open = next_json(&mut host).await;
    assert_eq!(open["t"], "conn_open");
    assert_eq!(open["kind"], "invite");
    assert_eq!(open["invite"], INVITE);
    let _second = device_socket(&relay, &identity, Some(&bearer(&invite_token)))
        .await
        .unwrap();
    assert_eq!(next_json(&mut host).await["t"], "conn_open");
    let error = device_socket(&relay, &identity, Some(&bearer(&invite_token)))
        .await
        .unwrap_err();
    assert_eq!(refusal(error).0, 401);

    // An expired invite: 401 invite_expired, and the relay deletes it.
    let other_token = [0x32u8; 32];
    let answer = control(
        &mut host,
        json!({"t": "invite_put", "rid": 4, "invite": "JJJJJJJJJJJJJJJJJJJJJJ", "hash": hash(&other_token), "exp": relay.now_ms() + 1_000, "tries": 5}),
    )
    .await;
    assert_eq!(answer["t"], "ok");
    relay.advance_clock(Duration::from_secs(2));
    let error = device_socket(&relay, &identity, Some(&bearer(&other_token)))
        .await
        .unwrap_err();
    assert_eq!(refusal(error).1.as_deref(), Some("invite_expired"));
    assert!(relay
        .invites(&identity.host_id())
        .iter()
        .all(|invite| invite.invite.as_str() != "JJJJJJJJJJJJJJJJJJJJJJ"));
}

#[tokio::test]
async fn paired_devices_forwarding_and_close_codes() {
    let relay = MockRelay::start().await.unwrap();
    let identity = HostIdentity::generate().unwrap();
    let mut host = host_socket(&relay, &identity, unix_time_ms()).await;
    let token_a = RelayToken::generate().unwrap();
    let token_b = RelayToken::generate().unwrap();
    for (rid, device, token) in [(1, DEVICE_A, &token_a), (2, DEVICE_B, &token_b)] {
        let answer = control(
            &mut host,
            json!({"t": "device_put", "rid": rid, "device": device, "hash": token.hash().as_str()}),
        )
        .await;
        assert_eq!(answer, json!({"t": "ok", "rid": rid}));
    }

    let mut device = device_socket(&relay, &identity, Some(&bearer(token_a.as_bytes())))
        .await
        .unwrap();
    let open = next_json(&mut host).await;
    assert_eq!(open["kind"], "device");
    assert_eq!(open["device"], DEVICE_A);
    let conn = open["conn"].as_u64().unwrap() as u32;

    // Device to host: a DATA frame with the connection id.
    device
        .send(Message::Binary(b"noise-1".to_vec().into()))
        .await
        .unwrap();
    let mut expected = vec![0x01];
    expected.extend_from_slice(&conn.to_be_bytes());
    expected.extend_from_slice(b"noise-1");
    assert_eq!(
        next_message(&mut host).await,
        Message::Binary(expected.into())
    );

    // Host to device: the Noise message without the header.
    let mut frame = vec![0x01];
    frame.extend_from_slice(&conn.to_be_bytes());
    frame.extend_from_slice(b"noise-2");
    host.send(Message::Binary(frame.into())).await.unwrap();
    assert_eq!(
        next_message(&mut device).await,
        Message::Binary(b"noise-2".to_vec().into())
    );

    // An unknown connection and an unknown frame type.
    host.send(Message::Binary(vec![0x01, 0, 0, 0, 7, 1].into()))
        .await
        .unwrap();
    assert_eq!(
        next_json(&mut host).await,
        json!({"t": "conn_close", "conn": 7, "code": 1006})
    );
    host.send(Message::Binary(vec![0x02, 0, 0, 0, 7, 1].into()))
        .await
        .unwrap();
    let answer = next_json(&mut host).await;
    assert_eq!(answer["t"], "err");
    assert_eq!(answer["code"], "bad_frame");

    // The text ping gets pong. Other text closes the socket with 4400.
    device.send(Message::text("ping")).await.unwrap();
    assert_eq!(next_message(&mut device).await, Message::text("pong"));
    device.send(Message::text("hello")).await.unwrap();
    assert_eq!(close_code(&mut device).await, Some(4400));
    assert_eq!(
        next_json(&mut host).await,
        json!({"t": "conn_close", "conn": conn, "code": 4400})
    );

    // A frame above 65,535 bytes closes the socket with 4413.
    let mut device = device_socket(&relay, &identity, Some(&bearer(token_a.as_bytes())))
        .await
        .unwrap();
    let conn = next_json(&mut host).await["conn"].as_u64().unwrap();
    device
        .send(Message::Binary(vec![0u8; MAX_DEVICE_FRAME + 1].into()))
        .await
        .unwrap();
    assert_eq!(close_code(&mut device).await, Some(4413));
    assert_eq!(
        next_json(&mut host).await,
        json!({"t": "conn_close", "conn": conn, "code": 4413})
    );

    // conn_close from the host closes the device socket, with no conn_close back.
    let mut device = device_socket(&relay, &identity, Some(&bearer(token_b.as_bytes())))
        .await
        .unwrap();
    let conn = next_json(&mut host).await["conn"].as_u64().unwrap();
    send_json(
        &mut host,
        json!({"t": "conn_close", "conn": conn, "code": 4410}),
    )
    .await;
    assert_eq!(close_code(&mut device).await, Some(4410));

    // A device that closes its socket: the host gets conn_close with the code.
    let mut device = device_socket(&relay, &identity, Some(&bearer(token_b.as_bytes())))
        .await
        .unwrap();
    let open = next_json(&mut host).await;
    assert_eq!(open["device"], DEVICE_B);
    let conn = open["conn"].as_u64().unwrap();
    device.close(None).await.unwrap();
    let closed = next_json(&mut host).await;
    assert_eq!(closed["t"], "conn_close");
    assert_eq!(closed["conn"], conn);

    // devices_sync removes device A and closes its socket with 4401.
    let mut device = device_socket(&relay, &identity, Some(&bearer(token_a.as_bytes())))
        .await
        .unwrap();
    let conn = next_json(&mut host).await["conn"].as_u64().unwrap();
    send_json(
        &mut host,
        json!({"t": "devices_sync", "rid": 9, "devices": [{"device": DEVICE_B, "hash": token_b.hash().as_str()}]}),
    )
    .await;
    assert_eq!(close_code(&mut device).await, Some(4401));
    let mut answers = [next_json(&mut host).await, next_json(&mut host).await];
    answers.sort_by_key(|answer| answer["t"].as_str().unwrap_or_default().to_owned());
    assert_eq!(
        answers[0],
        json!({"t": "conn_close", "conn": conn, "code": 4401})
    );
    assert_eq!(answers[1], json!({"t": "ok", "rid": 9}));
    assert_eq!(relay.devices(&identity.host_id()).len(), 1);
    let error = device_socket(&relay, &identity, Some(&bearer(token_a.as_bytes())))
        .await
        .unwrap_err();
    assert_eq!(refusal(error).0, 401);

    // A host DATA frame above 65,540 bytes closes the host socket with 4413,
    // and the relay closes every device socket with 4408.
    let mut device = device_socket(&relay, &identity, Some(&bearer(token_b.as_bytes())))
        .await
        .unwrap();
    next_json(&mut host).await;
    let mut frame = vec![0x01, 0, 0, 0, 1];
    frame.resize(65_541, 0);
    host.send(Message::Binary(frame.into())).await.unwrap();
    assert_eq!(close_code(&mut host).await, Some(4413));
    assert_eq!(close_code(&mut device).await, Some(4408));
    assert!(!relay.host_connected(&identity.host_id()));
}

#[tokio::test]
async fn limits_of_invites_devices_and_sockets() {
    let relay = MockRelay::start().await.unwrap();
    let identity = HostIdentity::generate().unwrap();
    let mut host = host_socket(&relay, &identity, unix_time_ms()).await;
    let exp = relay.now_ms() + 60_000;

    // At most 4 active invites.
    for index in 0..5u8 {
        let invite = format!("{}", (b'K' + index) as char).repeat(22);
        let answer = control(
            &mut host,
            json!({"t": "invite_put", "rid": index, "invite": invite, "hash": hash(&[index; 32]), "exp": exp, "tries": 1}),
        )
        .await;
        if index < 4 {
            assert_eq!(answer["t"], "ok");
        } else {
            assert_eq!(answer["code"], "invite_limit");
        }
    }

    // At most 32 paired devices.
    let mut tokens = Vec::new();
    for index in 0..33u32 {
        let device = format!("{index:0>22}");
        let token = [index as u8 + 100; 32];
        tokens.push(token);
        let answer = control(
            &mut host,
            json!({"t": "device_put", "rid": 100 + index, "device": device, "hash": hash(&token)}),
        )
        .await;
        if index < 32 {
            assert_eq!(answer["t"], "ok", "device {index}");
        } else {
            assert_eq!(answer["code"], "device_limit");
        }
    }
    let too_many: Vec<Value> = (0..33)
        .map(|index| json!({"device": format!("{index:0>22}"), "hash": hash(&[index as u8; 32])}))
        .collect();
    let answer = control(
        &mut host,
        json!({"t": "devices_sync", "rid": 200, "devices": too_many}),
    )
    .await;
    assert_eq!(answer["code"], "device_limit");

    // At most 8 open device sockets: the ninth gets 429.
    let mut sockets = Vec::new();
    for token in tokens.iter().take(8) {
        sockets.push(
            device_socket(&relay, &identity, Some(&bearer(token)))
                .await
                .unwrap(),
        );
        assert_eq!(next_json(&mut host).await["t"], "conn_open");
    }
    let error = device_socket(&relay, &identity, Some(&bearer(&tokens[8])))
        .await
        .unwrap_err();
    assert_eq!(refusal(error).0, 429);
    assert_eq!(relay.device_sockets(&identity.host_id()), 8);

    // A control frame above 65,536 bytes closes the host socket with 4413.
    let big = json!({"t": "invite_del", "rid": 1, "invite": "x".repeat(70_000)});
    send_json(&mut host, big).await;
    assert_eq!(close_code(&mut host).await, Some(4413));
}

#[tokio::test]
async fn upgrade_rate_limit() {
    let relay = MockRelay::start_with(MockRelayConfig {
        upgrade_rate_limit: Some(3),
        ..MockRelayConfig::default()
    })
    .await
    .unwrap();
    let identity = HostIdentity::generate().unwrap();
    for _ in 0..3 {
        let error = device_socket(&relay, &identity, None).await.unwrap_err();
        assert_eq!(refusal(error).0, 401);
    }
    let error = device_socket(&relay, &identity, None).await.unwrap_err();
    assert_eq!(refusal(error), (429, Some("rate_limited".to_owned()), None));
    // The host path has its own count.
    let _host = host_socket(&relay, &identity, unix_time_ms()).await;
}
