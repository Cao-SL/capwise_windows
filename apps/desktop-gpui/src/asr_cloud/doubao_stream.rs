use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::json;
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{
        Error, Message, client::IntoClientRequest as _, http::HeaderValue,
        protocol::WebSocketConfig,
    },
};

use super::{
    DoubaoResult, Failure, REQUEST_TIMEOUT, Request, SAMPLE_RATE, Utterance, doubao_options,
    spread_sentences, tidy,
};
use crate::{
    i18n::tr,
    teleprompter_follow::{BillingResource, FollowError, decode_response, packet},
};

pub(super) const DURATION: &str = BillingResource::Duration.id();
pub(super) const CONCURRENT: &str = BillingResource::Concurrent.id();
const FRAME_DURATION: Duration = Duration::from_millis(100);
const AUDIO_FRAME_BYTES: usize = SAMPLE_RATE * 2 / 5;
const MAX_REPLY_BYTES: usize = 1024 * 1024;

pub(super) fn selected(model: &str) -> bool {
    matches!(model.trim(), DURATION | CONCURRENT)
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum Results {
    One(DoubaoResult),
    Many(Vec<DoubaoResult>),
}

impl Results {
    fn merge(self) -> DoubaoResult {
        match self {
            Self::One(result) => result,
            Self::Many(results) => {
                let mut merged = DoubaoResult::default();
                for result in results {
                    if !result.text.is_empty() {
                        if !merged.text.is_empty() {
                            merged.text.push(' ');
                        }
                        merged.text.push_str(&result.text);
                    }
                    merged.utterances.extend(result.utterances);
                }
                merged
            }
        }
    }
}

pub(super) async fn recognize(
    request: &Request<'_>,
    wav: &[u8],
    duration_ms: f64,
) -> Result<Vec<Utterance>, Failure> {
    let name = (request.service.name)();
    tokio::time::timeout(REQUEST_TIMEOUT, recognize_stream(request, wav, duration_ms))
        .await
        .map_err(|error| Failure::Fatal(tr::asr::connection_failed(name, error.to_string())))?
}

async fn recognize_stream(
    request: &Request<'_>,
    wav: &[u8],
    duration_ms: f64,
) -> Result<Vec<Utterance>, Failure> {
    let name = (request.service.name)();
    let unreadable = || Failure::Fatal(tr::asr::unreadable(name));
    let base = request.endpoint.base_url.trim().trim_end_matches('/');
    if !crate::ai_store::address_allowed(base) {
        return Err(Failure::Fatal(tr::ai::address_invalid().to_string()));
    }
    let (scheme, base) = base.split_once("://").ok_or_else(unreadable)?;
    let ws_scheme = if scheme.eq_ignore_ascii_case("https") {
        "wss"
    } else {
        "ws"
    };
    let url = format!("{ws_scheme}://{base}/sauc/bigmodel_nostream");
    let mut upgrade = url.into_client_request().map_err(|_| unreadable())?;
    let resource = request.endpoint.model.trim();
    let request_id = uuid::Uuid::new_v4().to_string();
    for (name, value) in [
        ("x-api-key", request.key),
        ("x-api-resource-id", resource),
        ("x-api-request-id", request_id.as_str()),
        ("x-api-sequence", "-1"),
    ] {
        let mut value = value.parse::<HeaderValue>().map_err(|_| unreadable())?;
        value.set_sensitive(name == "x-api-key");
        upgrade.headers_mut().insert(name, value);
    }
    let socket_config = WebSocketConfig {
        max_message_size: Some(MAX_REPLY_BYTES),
        max_frame_size: Some(MAX_REPLY_BYTES),
        ..Default::default()
    };
    let (mut socket, _) = tokio::time::timeout(
        Duration::from_secs(15),
        connect_async_with_config(upgrade, Some(socket_config), true),
    )
    .await
    .map_err(|error| Failure::Retry(tr::asr::connection_failed(name, error.to_string())))?
    .map_err(|error| match error {
        Error::Http(response) => {
            let status = response.status();
            if status.as_u16() == 401 {
                Failure::Fatal(tr::asr::auth_failed(name))
            } else {
                let log_id = response
                    .headers()
                    .get("x-tt-logid")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default();
                let detail = if status.as_u16() == 403 {
                    tr::ai::asr::doubao::stream_access(
                        resource,
                        if log_id.is_empty() { "—" } else { log_id },
                    )
                } else {
                    format!("{resource} {log_id}").trim_end().to_string()
                };
                let message = tr::asr::failed(name, status.as_u16(), detail);
                if status.as_u16() == 429 || status.is_server_error() {
                    Failure::Retry(message)
                } else {
                    Failure::Fatal(message)
                }
            }
        }
        _ => Failure::Retry(tr::asr::connection_failed(name, "WebSocket handshake")),
    })?;
    let mut options = doubao_options("bigmodel", request.context);
    options["result_type"] = json!("full");
    let config = json!({
        "user": { "uid": "capwise" },
        "audio": { "format": "wav", "codec": "raw", "rate": SAMPLE_RATE, "bits": 16, "channel": 1 },
        "request": options
    });
    let send_error = || Failure::Fatal(tr::asr::connection_failed(name, "WebSocket audio upload"));
    let mut audio = wav.chunks(AUDIO_FRAME_BYTES).peekable();
    let mut latest = None;
    let mut next_frame = Some(packet(1, 0, 1, config.to_string().as_bytes()));
    let mut final_audio_sent = false;
    loop {
        let sent_at = tokio::time::Instant::now();
        if let Some(frame) = next_frame.take() {
            socket
                .send(Message::Binary(frame))
                .await
                .map_err(|_| send_error())?;
        }
        let reply = loop {
            match socket.next().await {
                Some(Ok(Message::Binary(data))) => break decode_response::<Results>(&data),
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {
                    socket.flush().await.map_err(|_| send_error())?;
                }
                _ => return Err(send_error()),
            }
        };
        let reply = match reply {
            Ok(reply) => reply,
            Err(FollowError::Server {
                code: 20_000_003, ..
            }) => return Ok(Vec::new()),
            Err(FollowError::Server { code, .. }) => {
                return Err(Failure::Fatal(tr::asr::failed(name, code, resource)));
            }
            Err(_) => return Err(unreadable()),
        };
        if let Some(result) = reply.result {
            latest = Some(result.merge());
        }
        if reply.final_packet {
            if !final_audio_sent {
                return Err(unreadable());
            }
            let mut result = latest.unwrap_or_default();
            if result.utterances.is_empty() {
                result.utterances = spread_sentences(&tidy(&result.text), duration_ms);
            }
            return Ok(result.utterances);
        }
        if let Some(bytes) = audio.next() {
            final_audio_sent = audio.peek().is_none();
            next_frame = Some(packet(2, if final_audio_sent { 2 } else { 0 }, 0, bytes));
            tokio::time::sleep_until(sent_at + FRAME_DURATION).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_store::Endpoint;

    #[tokio::test]
    async fn streaming_uses_saved_resource_and_keeps_word_times() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}/api/v3", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_hdr_async(
                stream,
                |request: &Request, response: Response| {
                    assert_eq!(request.uri().path(), "/api/v3/sauc/bigmodel_nostream");
                    assert_eq!(request.headers()["x-api-resource-id"], CONCURRENT);
                    assert_eq!(request.headers()["x-api-key"], "test-key");
                    Ok(response)
                },
            )
            .await
            .unwrap();

            let config = socket.next().await.unwrap().unwrap().into_data();
            assert_eq!(&config[..4], &[0x11, 0x10, 0x10, 0]);
            let config: serde_json::Value = serde_json::from_slice(&config[8..]).unwrap();
            assert_eq!(config["request"]["model_name"], "bigmodel");
            assert_eq!(config["request"]["show_utterances"], true);
            assert_eq!(config["audio"]["format"], "wav");
            socket
                .send(Message::Binary(packet(9, 0, 1, b"{}")))
                .await
                .unwrap();

            let audio = socket.next().await.unwrap().unwrap().into_data();
            assert_eq!(&audio[..4], &[0x11, 0x22, 0, 0]);
            assert_eq!(&audio[8..12], b"RIFF");
            socket
                .send(Message::Binary(packet(9, 0, 1, b"{}")))
                .await
                .unwrap();
            let result = br#"{"result":[{"text":"hi","utterances":[{"text":"hi","start_time":10,"end_time":80,"words":[{"text":"hi","start_time":20,"end_time":70}]}]}]}"#;
            socket
                .send(Message::Binary(packet(9, 2, 1, result)))
                .await
                .unwrap();
        });

        let endpoint = Endpoint {
            base_url,
            model: CONCURRENT.to_owned(),
        };
        let request = super::Request {
            service: super::super::service("doubao").unwrap(),
            endpoint: &endpoint,
            key: "test-key",
            context: "",
        };
        let audio = super::super::wav_bytes(&[0.0; 1600]);
        let result = match recognize(&request, &audio, 100.0).await {
            Ok(result) => result,
            Err(_) => panic!("Stream did not return results"),
        };
        server.await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].words[0].text, "hi");
        assert_eq!(result[0].words[0].start_time, 20.0);
    }
}
