//! `WsFeed` — a live JSON-RPC WebSocket `Feed` for Solana mainnet.
//!
//! All code in this module is gated on the `ws` feature. The default build
//! of `dl-feed` is async-free, so crates that only need the capture/replay
//! path don't pull in tokio.
//!
//! ## Architecture
//!
//! ```text
//!  ┌──────────────┐  commands (tokio mpsc)   ┌──────────────────────┐
//!  │   WsFeed     │ ───────────────────────▶ │  background tokio    │
//!  │  (caller)    │                          │  task: own the WS,   │
//!  │              │ ◀─────────────────────── │  parse → FeedEvent,  │
//!  │  events: std │   events (std mpsc)      │  blocking_send into  │
//!  │  mpsc recv   │                          │  the events channel  │
//!  └──────────────┘                          └──────────────────────┘
//! ```
//!
//! The bridge is necessary because [`dl_core::Feed::next_event`] is a sync
//! method, but tungstenite I/O is async. The background task does all the
//! async work; `next_event` does a `recv_timeout` on a synchronous channel.
//!
//! ## Reconnect policy
//!
//! The background task does NOT auto-reconnect on disconnect in v1.0.
//! Phase 2's primary input is replay-from-capture, so a dropped live stream
//! just means the rest of the session is gone; the consumer should treat
//! the next `next_event()` returning `None` as "stream ended, switch to
//! replay". Reconnect is a v1.1 concern.

#![cfg(feature = "ws")]

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::mpsc as tmpsc;
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
use tokio_tungstenite::WebSocketStream;

use dl_core::{Feed, FeedEvent};

use crate::error::FeedError;

/// Default capacity of the event channel between the background task and
/// the caller. Sized to absorb a few-second burst at full Solana slot rate
/// (1 event per 400ms × 32 accounts per slot × a few seconds) without
/// back-pressuring the WS reader.
const DEFAULT_EVENT_CAPACITY: usize = 4096;

/// Default timeout for `next_event()`.
const DEFAULT_NEXT_EVENT_TIMEOUT: Duration = Duration::from_millis(100);

/// Live JSON-RPC WebSocket feed for Solana.
pub struct WsFeed {
    /// Receiver end of the event channel. Background task sends via the
    /// paired `SyncSender`.
    events: Receiver<FeedEvent>,
    /// Sender end of the command channel. The background task receives
    /// subscribe_* commands and acts on the WS.
    commands: tmpsc::Sender<Command>,
    /// Timeout for `next_event`.
    next_event_timeout: Duration,
    /// Sticky-None flag.
    exhausted: bool,
}

enum Command {
    Account {
        pubkey: [u8; 32],
        reply: oneshot::Sender<Result<u64, FeedError>>,
    },
    Program {
        program: [u8; 32],
        reply: oneshot::Sender<Result<u64, FeedError>>,
    },
    Slots {
        reply: oneshot::Sender<Result<u64, FeedError>>,
    },
}

impl WsFeed {
    /// Connect to `url` and start the background reader task. `url` is the
    /// full WebSocket URL, including scheme (`ws://` or `wss://`).
    pub async fn connect(url: &str) -> Result<Self, FeedError> {
        let mut request = url
            .into_client_request()
            .map_err(|e| FeedError::Ws(format!("invalid request: {e}")))?;
        // Solana public RPCs don't require a Sec-WebSocket-Protocol header,
        // but private ones often do; let the caller wire one through by
        // not adding it here.
        let _ = request.headers_mut();

        let (ws, _response) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|e| FeedError::Ws(format!("connect failed: {e}")))?;

        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(DEFAULT_EVENT_CAPACITY);
        let (cmd_tx, cmd_rx) = tmpsc::channel(64);

        tokio::spawn(run_ws_task(ws, cmd_rx, event_tx));

        Ok(Self {
            events: event_rx,
            commands: cmd_tx,
            next_event_timeout: DEFAULT_NEXT_EVENT_TIMEOUT,
            exhausted: false,
        })
    }

    /// Override the default 100ms `next_event` timeout. Set to
    /// `Duration::ZERO` for non-blocking polling.
    pub fn set_next_event_timeout(&mut self, t: Duration) {
        self.next_event_timeout = t;
    }

    /// Subscribe to updates for a single account. Returns the subscription
    /// ID assigned by the RPC node.
    pub async fn subscribe_account(&mut self, pubkey: [u8; 32]) -> Result<u64, FeedError> {
        let (tx, rx) = oneshot::channel();
        self.commands
            .send(Command::Account { pubkey, reply: tx })
            .await
            .map_err(|_| FeedError::Ws("background task is gone".into()))?;
        rx.await
            .map_err(|_| FeedError::Ws("subscribe reply dropped".into()))?
    }

    /// Subscribe to updates for all accounts owned by a program. Returns
    /// the subscription ID.
    pub async fn subscribe_program(&mut self, program: [u8; 32]) -> Result<u64, FeedError> {
        let (tx, rx) = oneshot::channel();
        self.commands
            .send(Command::Program { program, reply: tx })
            .await
            .map_err(|_| FeedError::Ws("background task is gone".into()))?;
        rx.await
            .map_err(|_| FeedError::Ws("subscribe reply dropped".into()))?
    }

    /// Subscribe to slot updates. Returns the subscription ID.
    pub async fn subscribe_slots(&mut self) -> Result<u64, FeedError> {
        let (tx, rx) = oneshot::channel();
        self.commands
            .send(Command::Slots { reply: tx })
            .await
            .map_err(|_| FeedError::Ws("background task is gone".into()))?;
        rx.await
            .map_err(|_| FeedError::Ws("subscribe reply dropped".into()))?
    }
}

impl Feed for WsFeed {
    fn next_event(&mut self) -> Option<FeedEvent> {
        if self.exhausted {
            return None;
        }
        match self.events.recv_timeout(self.next_event_timeout) {
            Ok(ev) => Some(ev),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => {
                tracing::warn!("ws event channel disconnected; no more events");
                self.exhausted = true;
                None
            }
        }
    }
}

async fn run_ws_task(
    mut ws: WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    mut commands: tmpsc::Receiver<Command>,
    events: SyncSender<FeedEvent>,
) {
    let mut next_id: u64 = 0;
    // Maps JSON-RPC request id -> which subscription the response confirms.
    // Filled when we send a subscribe; consumed when the response arrives.
    let mut pending: HashMap<u64, PendingSub> = HashMap::new();
    // Maps subscription id (assigned by RPC) -> our handler for the
    // notifications. Filled when the response to a subscribe arrives.
    let mut subs: HashMap<u64, SubKind> = HashMap::new();

    loop {
        tokio::select! {
            cmd = commands.recv() => {
                let Some(cmd) = cmd else {
                    // WsFeed was dropped; close the WS and exit.
                    let _ = ws.close(None).await;
                    break;
                };
                match cmd {
                    Command::Account { pubkey, reply } => {
                        let id = next_id;
                        next_id += 1;
                        pending.insert(id, PendingSub { kind: SubKind::Account(pubkey) });
                        let msg = json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "method": "accountSubscribe",
                            "params": [
                                bs58::encode(pubkey).into_string(),
                                { "encoding": "base64", "commitment": "confirmed" }
                            ]
                        });
                        if let Err(e) = ws.send(Message::Text(msg.to_string())).await {
                            let _ = reply.send(Err(FeedError::Ws(format!("send failed: {e}"))));
                            pending.remove(&id);
                            break;
                        }
                        // We'll send Ok(sub_id) when the response arrives.
                        let reply_clone_tx = reply;
                        // Move the reply into a side-table so we can use it
                        // when the JSON-RPC response with that id arrives.
                        // For simplicity, we use a separate oneshot map:
                        pending_replies().lock().unwrap().insert(id, reply_clone_tx);
                    }
                    Command::Program { program, reply } => {
                        let id = next_id;
                        next_id += 1;
                        pending.insert(id, PendingSub { kind: SubKind::Program(program) });
                        let msg = json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "method": "programSubscribe",
                            "params": [
                                bs58::encode(program).into_string(),
                                { "encoding": "base64", "commitment": "confirmed" }
                            ]
                        });
                        if let Err(e) = ws.send(Message::Text(msg.to_string())).await {
                            let _ = reply.send(Err(FeedError::Ws(format!("send failed: {e}"))));
                            pending.remove(&id);
                            break;
                        }
                        pending_replies().lock().unwrap().insert(id, reply);
                    }
                    Command::Slots { reply } => {
                        let id = next_id;
                        next_id += 1;
                        pending.insert(id, PendingSub { kind: SubKind::Slots });
                        let msg = json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "method": "slotSubscribe",
                            "params": []
                        });
                        if let Err(e) = ws.send(Message::Text(msg.to_string())).await {
                            let _ = reply.send(Err(FeedError::Ws(format!("send failed: {e}"))));
                            pending.remove(&id);
                            break;
                        }
                        pending_replies().lock().unwrap().insert(id, reply);
                    }
                }
            }

            msg = ws.next() => {
                let Some(msg) = msg else { break; };
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Err(e) = handle_text(&text, &mut pending, &mut subs, &events) {
                            tracing::error!(error = %e, text = %text, "ws message handler error");
                        }
                    }
                    Ok(Message::Ping(p)) => {
                        if let Err(e) = ws.send(Message::Pong(p)).await {
                            tracing::error!(error = %e, "ws pong send failed");
                            break;
                        }
                    }
                    Ok(Message::Close(frame)) => {
                        tracing::warn!(?frame, "ws closed by peer");
                        break;
                    }
                    Ok(Message::Binary(_)) | Ok(Message::Frame(_)) | Ok(Message::Pong(_)) => {}
                    Err(e) => {
                        tracing::error!(error = %e, "ws read error");
                        break;
                    }
                }
            }
        }
    }
    // On exit, drop the event sender so the caller sees Disconnected.
    drop(events);
}

struct PendingSub {
    #[allow(dead_code)]
    kind: SubKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SubKind {
    Account([u8; 32]),
    Program([u8; 32]),
    Slots,
}

/// Side-table for oneshot senders indexed by JSON-RPC request id. Lives in
/// a global Mutex for simplicity — there is exactly one task per WsFeed,
/// and the contention surface is tiny (one entry per subscribe call).
type ReplyMap = std::sync::Mutex<HashMap<u64, oneshot::Sender<Result<u64, FeedError>>>>;

fn pending_replies() -> &'static ReplyMap {
    use std::sync::OnceLock;
    static REPLIES: OnceLock<ReplyMap> = OnceLock::new();
    REPLIES.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

fn handle_text(
    text: &str,
    pending: &mut HashMap<u64, PendingSub>,
    subs: &mut HashMap<u64, SubKind>,
    events: &SyncSender<FeedEvent>,
) -> Result<(), FeedError> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| FeedError::Ws(format!("invalid JSON: {e}")))?;

    // Two message shapes:
    //   1. Response:  { "id": N, "result": <sub_id> | null, "error": {...}? }
    //   2. Notification: { "method": "...", "params": { "subscription": N, "result": {...} } }
    if value.get("method").is_some() {
        handle_notification(&value, subs, events)?;
    } else if let Some(id) = value.get("id").and_then(|v| v.as_u64()) {
        handle_response(id, &value, pending, subs)?;
    } else {
        tracing::warn!(text, "ws message has no method and no id; ignoring");
    }
    Ok(())
}

fn handle_response(
    id: u64,
    value: &serde_json::Value,
    pending: &mut HashMap<u64, PendingSub>,
    subs: &mut HashMap<u64, SubKind>,
) -> Result<(), FeedError> {
    let reply = pending_replies().lock().unwrap().remove(&id);
    if let Some(error) = value.get("error") {
        let message = error
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("(no message)")
            .to_string();
        let code = error.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
        pending.remove(&id);
        if let Some(reply) = reply {
            let _ = reply.send(Err(FeedError::Rpc { code, message }));
        }
        return Ok(());
    }
    let sub_id = match value.get("result").and_then(|v| v.as_u64()) {
        Some(n) => n,
        None => {
            if let Some(reply) = reply {
                let _ = reply.send(Err(FeedError::Ws("response missing result".into())));
            }
            pending.remove(&id);
            return Ok(());
        }
    };
    if let Some(p) = pending.remove(&id) {
        subs.insert(sub_id, p.kind);
    }
    if let Some(reply) = reply {
        let _ = reply.send(Ok(sub_id));
    }
    Ok(())
}

fn handle_notification(
    value: &serde_json::Value,
    subs: &HashMap<u64, SubKind>,
    events: &SyncSender<FeedEvent>,
) -> Result<(), FeedError> {
    let method = value.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = value
        .get("params")
        .ok_or_else(|| FeedError::Ws("no params".into()))?;
    let sub_id = params
        .get("subscription")
        .and_then(|s| s.as_u64())
        .ok_or_else(|| FeedError::Ws("no subscription id".into()))?;
    let kind = match subs.get(&sub_id) {
        Some(k) => *k,
        None => {
            tracing::warn!(sub_id, method, "notification for unknown subscription");
            return Ok(());
        }
    };
    let result = params
        .get("result")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    match (method, kind) {
        ("accountNotification", SubKind::Account(pubkey)) => {
            // Result shape: { "context": { "slot": N }, "value": { "data": [base64str, "base64"], "lamports": N, ... } }
            let slot = result
                .pointer("/context/slot")
                .and_then(|s| s.as_u64())
                .unwrap_or(0);
            let data_b64 = result
                .pointer("/value/data/0")
                .and_then(|s| s.as_str())
                .unwrap_or("");
            let data = base64_decode(data_b64).unwrap_or_default();
            let ev = FeedEvent::AccountUpdate { slot, pubkey, data };
            let _ = events.send(ev);
        }
        ("accountNotification", SubKind::Program(_)) => {
            // Program notifications have a different shape; we don't decode
            // them in v1.0 (constant-product only). Log and skip.
            tracing::debug!(method, "program account notification ignored in v1.0");
        }
        ("slotNotification", SubKind::Slots) => {
            // Result shape: { "slot": N, "parent": N, "root": N }
            let slot = result.get("slot").and_then(|s| s.as_u64()).unwrap_or(0);
            let ev = FeedEvent::Slot { slot };
            let _ = events.send(ev);
        }
        (m, k) => {
            tracing::debug!(method = m, kind = ?k, "unhandled ws notification shape");
        }
    }
    Ok(())
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    use std::str;
    // Minimal base64 (standard alphabet, no padding required) — we accept
    // either padded or unpadded input because Solana emits the unpadded form.
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    for &b in s.as_bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => continue,
            _ => return None,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xFF) as u8);
        }
    }
    let _ = str::from_utf8(&out).ok(); // discard; not used as text
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_decodes_solana_style_unpadded() {
        // Solana account data is base64 with no padding. "AAAA" → 3 zero bytes.
        let v = base64_decode("AAAA").unwrap();
        assert_eq!(v, vec![0, 0, 0]);
        let v = base64_decode("AQID").unwrap(); // 1, 2, 3
        assert_eq!(v, vec![1, 2, 3]);
        // Padded form also works.
        let v = base64_decode("AQIDBA==").unwrap();
        assert_eq!(v, vec![1, 2, 3, 4]);
    }
}
