//! The device side of a pairing (section 7.3).

use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use zeroize::Zeroizing;

use super::{pair_error_from_connect, ClientConfig, DeviceInfo, PairError, PairedHost};
use crate::framing::Transport;
use crate::ids::{HostId, RequestId};
use crate::keys::DeviceKeys;
use crate::net::{self, Incoming, Outgoing, WsReader};
use crate::noise::{ConfirmationCode, HandshakePayload, PairingInitiator};
use crate::pairing::PairingOffer;
use crate::relay_auth::bearer_header_value;
use crate::relay_wire::{device_socket_url, CloseCode};
use crate::rpc::{self, ErrorCode, Message, PairRequest, PairRequestParams, MAX_DEVICE_NAME_CHARS};
use crate::unix_time_ms;

/// A pairing after the handshake, while the host asks the user.
///
/// Show [`PendingPairing::confirmation_code`] while [`PendingPairing::finish`] waits.
/// The user checks that the desktop shows the same code.
/// Dropping the value closes the connection.
pub struct PendingPairing {
    reader: WsReader,
    out: mpsc::Sender<Outgoing>,
    transport: Transport,
    code: ConfirmationCode,
    relay_url: String,
    host_id: HostId,
    host_noise_public: [u8; 32],
    offer_host_name: Option<String>,
    keys: DeviceKeys,
    device: DeviceInfo,
    response_timeout: Duration,
}

impl std::fmt::Debug for PendingPairing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingPairing")
            .field("code", &self.code)
            .field("host_id", &self.host_id)
            .finish_non_exhaustive()
    }
}

impl Drop for PendingPairing {
    fn drop(&mut self) {
        let _ = self.out.try_send(Outgoing::Close(1000, "pairing ended"));
    }
}

/// Starts a pairing from an offer URI: connect with the invite token, then run the handshake.
///
/// Call it only for text from the in-app QR scanner or for a pasted code, never for a deep link
/// (section 5.1).
pub async fn start_pairing(
    offer_uri: &str,
    device: DeviceInfo,
    config: &ClientConfig,
) -> Result<PendingPairing, PairError> {
    let chars = device.name.chars().count();
    if !(1..=MAX_DEVICE_NAME_CHARS).contains(&chars) {
        return Err(PairError::InvalidDeviceName);
    }
    let offer =
        PairingOffer::parse_uri(offer_uri, unix_time_ms()).map_err(PairError::InvalidOffer)?;
    let keys = DeviceKeys::generate().map_err(|error| PairError::Protocol(error.to_string()))?;

    let bearer = bearer_header_value(offer.invite_token().as_bytes());
    let header = net::secret_header(&bearer)
        .ok_or_else(|| PairError::Protocol("invalid bearer header".to_owned()))?;
    let url = device_socket_url(offer.relay_url(), offer.host_id());
    let socket = net::connect(&url, vec![(AUTHORIZATION, header)], config.connect_timeout)
        .await
        .map_err(pair_error_from_connect)?;
    let (mut reader, out, writer) = net::spawn_writer(socket, config.ping_interval);

    let payload = HandshakePayload {
        v: config.max_version,
        min: config.min_version,
        app: device.app.clone(),
    };
    let (initiator, message_1) = PairingInitiator::start(&offer, &keys, payload)
        .map_err(|error| PairError::Protocol(error.to_string()))?;
    out.send(Outgoing::Frame(WsMessage::Binary(message_1.into())))
        .await
        .map_err(|_| PairError::Network("the connection closed".to_owned()))?;
    let deadline = Instant::now() + config.handshake_timeout;
    let message_2 = match read_binary(&mut reader, deadline).await {
        Ok(message) => message,
        Err(error) => {
            writer.abort();
            return Err(error);
        }
    };
    let established = match initiator.read_message_2(&message_2) {
        Ok(established) => established,
        Err(error) => {
            writer.abort();
            return Err(PairError::Protocol(error.to_string()));
        }
    };
    if !established.versions_overlap() {
        let _ = out.try_send(Outgoing::Close(1000, "incompatible"));
        return Err(PairError::Incompatible);
    }
    Ok(PendingPairing {
        reader,
        out,
        code: established.confirmation_code(),
        transport: established.transport,
        relay_url: offer.relay_url().to_owned(),
        host_id: offer.host_id().clone(),
        host_noise_public: *offer.host_noise_public(),
        offer_host_name: offer.host_name().map(str::to_owned),
        keys,
        device,
        response_timeout: config.pair_request_timeout,
    })
}

/// Pairs with a host: [`start_pairing`], then `on_code`, then [`PendingPairing::finish`].
///
/// `on_code` gets the confirmation code before the host asks the user.
pub async fn pair(
    offer_uri: &str,
    device: DeviceInfo,
    config: &ClientConfig,
    on_code: impl FnOnce(ConfirmationCode) + Send,
) -> Result<PairedHost, PairError> {
    let pending = start_pairing(offer_uri, device, config).await?;
    on_code(pending.confirmation_code());
    pending.finish().await
}

impl PendingPairing {
    /// Returns the confirmation code of section 7.3.1. The phone shows it while it waits.
    pub fn confirmation_code(&self) -> ConfirmationCode {
        self.code
    }

    /// Returns the host id of the offer.
    pub fn host_id(&self) -> &HostId {
        &self.host_id
    }

    /// Returns the host name of the offer, if it had one.
    pub fn host_name(&self) -> Option<&str> {
        self.offer_host_name.as_deref()
    }

    /// Sends `pair.request` and waits for the answer of the user (130 seconds at most).
    pub async fn finish(mut self) -> Result<PairedHost, PairError> {
        let params = PairRequestParams {
            name: self.device.name.clone(),
            platform: self.device.platform.clone(),
            model: self.device.model.clone(),
            token: self.keys.relay_token().clone(),
            app: self.device.app.clone(),
        };
        // The encoded request holds the relay token, so it zeroizes on drop.
        let request = rpc::encode_call::<PairRequest>(RequestId::MIN, &params)
            .map(Zeroizing::new)
            .map_err(|error| PairError::Protocol(error.to_string()))?;
        let sealed = self
            .transport
            .seal(&request)
            .map_err(|error| PairError::Protocol(error.to_string()))?;
        for noise_message in sealed {
            self.out
                .send(Outgoing::Frame(WsMessage::Binary(noise_message.into())))
                .await
                .map_err(|_| PairError::Network("the connection closed".to_owned()))?;
        }
        let deadline = Instant::now() + self.response_timeout;
        let result = loop {
            let bytes = read_binary(&mut self.reader, deadline).await?;
            let message = match self.transport.open(&bytes) {
                Ok(Some(message)) => message,
                Ok(None) => continue,
                Err(error) => return Err(PairError::Protocol(error.to_string())),
            };
            match Message::decode(&message) {
                Ok(Message::Response(response)) if response.id == RequestId::MIN => {
                    break response
                        .decode_result::<PairRequest>()
                        .map_err(|error| PairError::Protocol(error.to_string()))?;
                }
                Ok(_) => continue,
                Err(error) => return Err(PairError::Protocol(error.to_string())),
            }
        };
        let result = result.map_err(|error| match error.code {
            ErrorCode::Denied => PairError::Denied,
            ErrorCode::Timeout => PairError::Timeout,
            ErrorCode::Busy => PairError::Busy,
            ErrorCode::Expired => PairError::Expired,
            _ => PairError::Rejected(error),
        })?;
        // The device closes the socket (section 7.3, step 10).
        let _ = self.out.try_send(Outgoing::Close(1000, "paired"));
        Ok(PairedHost {
            relay_url: std::mem::take(&mut self.relay_url),
            host_id: self.host_id.clone(),
            host_noise_public: self.host_noise_public,
            host_name: result.host,
            device_id: result.device,
            keys: self.keys.clone(),
        })
    }
}

/// Reads the next binary frame before `deadline`.
async fn read_binary(reader: &mut WsReader, deadline: Instant) -> Result<Vec<u8>, PairError> {
    loop {
        let frame = tokio::time::timeout_at(deadline, net::read_frame(reader))
            .await
            .map_err(|_| PairError::Network("the desktop did not answer in time".to_owned()))?;
        match frame {
            Incoming::Binary(bytes) => return Ok(bytes.to_vec()),
            Incoming::Text(_) | Incoming::Alive => continue,
            Incoming::Closed(code) => return Err(pair_error_from_close(code)),
            Incoming::Failed(reason) => return Err(PairError::Network(reason)),
        }
    }
}

/// Maps a close of the pairing socket.
///
/// `4426` comes from the untrusted relay. Only the own handshake gives
/// [`PairError::Incompatible`], so `4426` is a network error like `4400` (section 6.6).
fn pair_error_from_close(code: Option<u16>) -> PairError {
    match code.and_then(CloseCode::from_code) {
        Some(CloseCode::Unauthorized) => PairError::NotAuthorized,
        Some(CloseCode::RateLimited) => PairError::RateLimited,
        Some(CloseCode::HostGone) => PairError::HostOffline,
        _ => PairError::Network(match code {
            Some(code) => format!("the connection closed with code {code}"),
            None => "the connection closed".to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_4426_alone_is_not_incompatible() {
        assert_eq!(
            pair_error_from_close(Some(4426)),
            PairError::Network("the connection closed with code 4426".to_owned())
        );
        assert_eq!(pair_error_from_close(Some(4401)), PairError::NotAuthorized);
        assert_eq!(pair_error_from_close(Some(4408)), PairError::HostOffline);
    }
}
