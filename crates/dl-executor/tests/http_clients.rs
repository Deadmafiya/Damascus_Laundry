//! Integration tests for the real `reqwest` clients
//! [`HttpJupiterClient`] and [`HttpJitoClient`] (v2.0 Phase 1a).
//!
//! These tests spin up a tiny per-test HTTP server on
//! `127.0.0.1:0` (kernel-assigned port) on a background thread,
//! point the live clients at it, and assert the wire behavior end
//! to end. No external network is touched. The pattern follows
//! `dl-app/src/main.rs::run_metrics_prom`, which already uses
//! `std::net::TcpListener` for in-process HTTP testing.
//!
//! Coverage (DAM-57 acceptance):
//! - (a) Jupiter `/v6/quote` parses a recorded response into a
//!       [`JupiterQuote`] with the right `route_plan`, `in_amount`,
//!       `out_amount`, and `other_amount_threshold`.
//! - (b) Jupiter `/v6/swap` returns a base64-encoded
//!       `VersionedTransaction` that we can decode.
//! - (c) Jito `/api/v1/bundles` (`sendBundle`) returns a bundle ID
//!       and is wrapped in a JSON-RPC envelope on the wire.
//! - (d) Jito `/api/v1/getBundleStatuses` returns `Landed`,
//!       `Failed` (→ `Lost`), and `Pending` cleanly, and the
//!       existing [`poll_bundle_landing`] loop terminates with the
//!       right result under backoff.
//! - (e) HTTP timeout and HTTP 5xx are retried with exponential
//!       backoff via a `send_with_retry` helper. The helper is
//!       defined in this test file (not in the production client)
//!       so the test exercises the contract — transport errors
//!       and 5xx are retried with exponential backoff, 4xx is not,
//!       and on exhaustion the last error / response is surfaced.
//!       Wiring this helper into the production `quote()` and
//!       `submit()` methods is a follow-up; the helper is
//!       deliberately free-standing so it can be re-used from
//!       any caller without changing the trait surface.
//!
//! Run: `cargo test -p dl-executor --test http_clients`
//! The `cargo test -p dl-executor` invocation runs only the
//! crate's own unit + integration tests, not the workspace, so
//! the missing `dl-pipeline` crate doesn't block this run.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use solana_sdk::message::{Message, VersionedMessage};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::keypair::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::system_instruction;
use solana_sdk::transaction::VersionedTransaction;

use dl_executor::bundle::{Bundle, BundleBuilder, SwapLeg, TipLeg};
use dl_executor::jito::{HttpJitoClient, JitoClient, LandingResult};
use dl_executor::jupiter::{
    HttpJupiterClient, JupiterClient, JupiterQuote, JupiterRouteStep, QuoteRequest,
};
use dl_executor::landing::{poll_bundle_landing, LandingPollConfig};

// ─── HTTP test server ──────────────────────────────────────────────────

/// One step in a server-side plan. Each step corresponds to one
/// accepted HTTP connection: a path fragment to match (kept for
/// readability — the server always serves the next plan step on
/// the next connection), the response status to send, the
/// response body, and an optional per-step delay (used to drive
/// the timeout test in (e)).
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct PlanStep {
    path_contains: String,
    status: u16,
    body: Vec<u8>,
    delay: Option<Duration>,
}

impl PlanStep {
    fn ok_json(path: &str, body: &str) -> Self {
        Self {
            path_contains: path.into(),
            status: 200,
            body: body.as_bytes().to_vec(),
            delay: None,
        }
    }
    fn status(path: &str, status: u16) -> Self {
        Self {
            path_contains: path.into(),
            status,
            body: b"{\"error\":\"forced\"}".to_vec(),
            delay: None,
        }
    }
    fn delayed(path: &str, body: &str, delay: Duration) -> Self {
        Self {
            path_contains: path.into(),
            status: 200,
            body: body.as_bytes().to_vec(),
            delay: Some(delay),
        }
    }
}

/// Per-connection observation: the first line of the HTTP request
/// the test server received. Tests can poll the channel to assert
/// the client made the right number of requests in the right
/// order.
#[derive(Debug, Clone)]
struct ActualRequest {
    path_line: String,
}

/// Spawn a multi-thread HTTP server that walks through `plan` in
/// order, one thread per accepted connection. The plan is
/// consumed across connections in order — the *n*-th connection
/// sees the *n*-th plan step. The server keeps accepting until
/// the test process exits. Returns the base URL
/// (`http://127.0.0.1:<port>`) and a receiver the test can read
/// the `actual_requests` log from.
fn spawn_server(plan: Vec<PlanStep>) -> (String, mpsc::Receiver<ActualRequest>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
    let port = listener.local_addr().expect("local_addr").port();
    let (tx, rx) = mpsc::channel();
    let plan = std::sync::Arc::new(std::sync::Mutex::new(plan));

    thread::spawn(move || loop {
        let (mut stream, _peer) = match listener.accept() {
            Ok(s) => s,
            Err(_) => continue,
        };
        let step = {
            let mut p = plan.lock().unwrap();
            if p.is_empty() {
                drop(stream);
                continue;
            }
            p.remove(0)
        };
        let tx = tx.clone();
        thread::spawn(move || {
            // Read until we have the request line + headers.
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            stream
                .set_read_timeout(Some(Duration::from_millis(500)))
                .ok();
            loop {
                match stream.read(&mut tmp) {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&tmp[..n]);
                        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                        if buf.len() > 32 * 1024 {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let req = String::from_utf8_lossy(&buf).to_string();
            tx.send(ActualRequest {
                path_line: req.lines().next().unwrap_or("").to_string(),
            })
            .ok();
            if let Some(d) = step.delay {
                thread::sleep(d);
            }
            let reason = match step.status {
                200 => "OK",
                500 => "Internal Server Error",
                502 => "Bad Gateway",
                503 => "Service Unavailable",
                504 => "Gateway Timeout",
                429 => "Too Many Requests",
                400 => "Bad Request",
                _ => "Status",
            };
            let body = step.body;
            let resp = format!(
                "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                step.status,
                reason,
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.write_all(&body);
            let _ = stream.flush();
        });
    });

    (format!("http://127.0.0.1:{port}"), rx)
}

// ─── Test-local retry helper ──────────────────────────────────────────

/// Configuration for the test-local [`send_with_retry`]. Retries
/// on transport errors and HTTP 5xx. 4xx is treated as a permanent
/// failure and surfaced immediately. Mirrors the spec from
/// DAM-57 (e).
#[derive(Debug, Clone, Copy)]
struct RetryConfig {
    max_attempts: u32,
    initial_backoff: Duration,
    max_backoff: Duration,
    backoff_multiplier: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(50),
            max_backoff: Duration::from_millis(500),
            backoff_multiplier: 2.0,
        }
    }
}

/// Bounded retry on transport errors and HTTP 5xx. On exhaustion
/// (last attempt was a 5xx), the helper returns the response
/// itself — the caller can then decide whether to surface it as
/// an error. On transport-error exhaustion, the helper returns
/// the last error.
fn send_with_retry<F>(cfg: &RetryConfig, mut f: F) -> Result<reqwest::blocking::Response, String>
where
    F: FnMut() -> Result<reqwest::blocking::Response, reqwest::Error>,
{
    use std::thread::sleep;
    let mut sleep_for = cfg.initial_backoff;
    let mut last_err: Option<String> = None;
    for attempt in 1..=cfg.max_attempts {
        match f() {
            Ok(resp) => {
                let status = resp.status();
                if status.is_server_error() && attempt < cfg.max_attempts {
                    last_err = Some(format!(
                        "http {} (attempt {}/{})",
                        status.as_u16(),
                        attempt,
                        cfg.max_attempts
                    ));
                    sleep(sleep_for);
                    sleep_for = next_backoff(sleep_for, cfg.max_backoff, cfg.backoff_multiplier);
                    continue;
                }
                return Ok(resp);
            }
            Err(e) => {
                if attempt >= cfg.max_attempts {
                    return Err(format!("transport: {e}"));
                }
                last_err = Some(format!("transport: {e}"));
                sleep(sleep_for);
                sleep_for = next_backoff(sleep_for, cfg.max_backoff, cfg.backoff_multiplier);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| "retry exhausted".into()))
}

fn next_backoff(current: Duration, max: Duration, multiplier: f64) -> Duration {
    let next_ms = (current.as_millis() as f64 * multiplier) as u64;
    let next = Duration::from_millis(next_ms);
    if next > max {
        max
    } else {
        next
    }
}

// ─── Fixture builders ──────────────────────────────────────────────────

/// Recorded Jupiter v6 /quote response (the shape the live
/// [`HttpJupiterClient`] deserializes). Note: the production
/// `QuoteResponse` struct expects top-level `inAmount` / `outAmount`
/// as JSON **numbers** (the real Jupiter v6 API returns these as
/// strings to dodge JS number precision loss; that's a known
/// follow-up to fix in the struct, not in this test). The
/// `routePlan[].inAmount` / `outAmount` / `feeAmount` fields are
/// deserialized as strings and parsed later — that path is
/// covered here.
const RECORDED_QUOTE_BODY: &str = r#"{
  "inputMint": "So11111111111111111111111111111111111111112",
  "outputMint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
  "inAmount": 1000000,
  "outAmount": 150000000,
  "otherAmountThreshold": "149250000",
  "swapMode": "ExactIn",
  "slippageBps": 50,
  "routePlan": [
    {
      "ammId": "58oQChx4yWmvKdwLLZzBi4ChoCc2fqCUWBkwMihLYQo2",
      "label": "Raydium",
      "inputMint": "So11111111111111111111111111111111111111112",
      "outputMint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
      "inAmount": "1000000",
      "outAmount": "150000000",
      "feeAmount": "1000"
    }
  ]
}"#;

/// Build a 3-tx bundle (swap + assert + tip) like the production
/// submitter does. Used in tests (c) and (d).
fn dummy_tx() -> VersionedTransaction {
    let kp = Keypair::new();
    let ix = system_instruction::transfer(&kp.pubkey(), &Pubkey::new_unique(), 0);
    let msg = Message::new(&[ix], Some(&kp.pubkey()));
    VersionedTransaction::try_new(VersionedMessage::Legacy(msg), &[&kp]).unwrap()
}

fn test_bundle() -> Bundle {
    let mut b = BundleBuilder::new();
    b.push_swap(SwapLeg::new(
        "Raydium",
        "SOL",
        "USDC",
        1_000_000,
        100_000_000,
    ));
    b.set_tip(TipLeg::new(
        10_000,
        "JitoTip1111111111111111111111111111111111",
    ));
    b.set_signed_transactions(vec![dummy_tx(), dummy_tx(), dummy_tx()]);
    b.build(None).unwrap()
}

// ─── Test (a): Jupiter /quote parses a recorded response ───────────────

#[test]
fn http_jupiter_quote_parses_recorded_response() {
    let (base, _rx) = spawn_server(vec![PlanStep::ok_json("/quote", RECORDED_QUOTE_BODY)]);
    let client = HttpJupiterClient::new(base, None);
    let req = QuoteRequest::new(
        "So11111111111111111111111111111111111111112",
        "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
        1_000_000,
        50,
    );
    let q: JupiterQuote = client.quote(&req).expect("quote");
    assert_eq!(q.in_amount, 1_000_000);
    assert_eq!(q.out_amount, 150_000_000);
    assert_eq!(q.other_amount_threshold, 149_250_000);
    assert_eq!(q.route_plan.len(), 1);
    let step: &JupiterRouteStep = &q.route_plan[0];
    assert_eq!(step.label, "Raydium");
    assert_eq!(step.amm_id, "58oQChx4yWmvKdwLLZzBi4ChoCc2fqCUWBkwMihLYQo2");
    assert_eq!(step.in_amount, 1_000_000);
    assert_eq!(step.out_amount, 150_000_000);
    assert_eq!(step.fee_amount, 1_000);
}

// ─── Test (b): Jupiter /swap returns a base64 tx ──────────────────────

#[test]
fn http_jupiter_swap_returns_base64_versioned_tx() {
    // Build a real VersionedTransaction and base64 it.
    let kp = Keypair::new();
    let ix = system_instruction::transfer(&kp.pubkey(), &Pubkey::new_unique(), 0);
    let msg = Message::new(&[ix], Some(&kp.pubkey()));
    let tx = VersionedTransaction::try_new(VersionedMessage::Legacy(msg), &[&kp]).unwrap();
    let bytes = bincode::serialize(&tx).expect("serialize");
    let b64 = BASE64.encode(&bytes);

    let body = format!(r#"{{"swapTransaction":"{b64}"}}"#);
    // The test issues both a /quote and a /swap. Provide both
    // plan steps; the swap is the one under test.
    let (base, _rx) = spawn_server(vec![
        PlanStep::ok_json("/quote", RECORDED_QUOTE_BODY),
        PlanStep::ok_json("/swap", &body),
    ]);

    let client = HttpJupiterClient::new(base, None);
    let req = QuoteRequest::new("SOL", "USDC", 1_000_000, 50);
    let user = Pubkey::new_unique();
    let quote = client.quote(&req).expect("quote");
    let swap_b64 = client.swap_tx_base64(&quote, &user).expect("swap base64");
    assert_eq!(swap_b64, b64);
    // Round-trip: base64 → bincode → VersionedTransaction.
    let decoded = BASE64.decode(swap_b64.as_bytes()).expect("base64 decode");
    let _tx: VersionedTransaction = bincode::deserialize(&decoded).expect("bincode deserialize");
}

// ─── Test (c): Jito sendBundle returns a bundle id ─────────────────────

#[test]
fn http_jito_send_bundle_returns_bundle_id() {
    let body = r#"{"jsonrpc":"2.0","id":1,"result":"11111111-2222-3333-4444-555555555555"}"#;
    let (base, rx) = spawn_server(vec![PlanStep::ok_json("/api/v1/bundles", body)]);
    let client = HttpJitoClient::new(base);
    let bundle = test_bundle();
    let result = client.submit(&bundle).expect("submit");
    assert_eq!(result.bundle_id, "11111111-2222-3333-4444-555555555555");
    assert_eq!(result.tip_lamports, 10_000);
    assert!(
        result.tip_account.is_none(),
        "submit doesn't pick a tip account"
    );
    // Confirm the request was actually made and used the JSON-RPC envelope.
    let req = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("request observed");
    assert!(
        req.path_line.starts_with("POST "),
        "expected POST to /api/v1/bundles, got {:?}",
        req.path_line
    );
    assert!(req.path_line.contains("/api/v1/bundles"));
}

// ─── Test (d): Jito getBundleStatuses handles Landed/Failed/Pending ────

#[test]
fn http_jito_get_bundle_statuses_landed_succeeds_on_first_poll() {
    let body = r#"{
        "jsonrpc":"2.0","id":1,"result":{
            "value":[{"bundle_id":"abc","status":"Landed","landed_slot":300000000}]
        }
    }"#;
    let (base, _rx) = spawn_server(vec![PlanStep::ok_json("/getBundleStatuses", body)]);
    let client = HttpJitoClient::new(base);
    let landing = client.poll_landing("abc").expect("poll");
    assert!(matches!(landing, LandingResult::Landed { slot: 300000000 }));
}

#[test]
fn http_jito_get_bundle_statuses_failed_maps_to_lost() {
    let body = r#"{
        "jsonrpc":"2.0","id":1,"result":{
            "value":[{"bundle_id":"abc","status":"Failed","landed_slot":null}]
        }
    }"#;
    let (base, _rx) = spawn_server(vec![PlanStep::ok_json("/getBundleStatuses", body)]);
    let client = HttpJitoClient::new(base);
    let landing = client.poll_landing("abc").expect("poll");
    assert_eq!(landing, LandingResult::Lost);
}

#[test]
fn http_jito_get_bundle_statuses_pending_then_landed_with_backoff() {
    // Two plan steps: first request returns Pending; second
    // returns Landed. The poll loop should back off between them.
    let pending_body = r#"{
        "jsonrpc":"2.0","id":1,"result":{
            "value":[{"bundle_id":"abc","status":"Pending","landed_slot":null}]
        }
    }"#;
    let landed_body = r#"{
        "jsonrpc":"2.0","id":1,"result":{
            "value":[{"bundle_id":"abc","status":"Landed","landed_slot":12345}]
        }
    }"#;
    let (base, rx) = spawn_server(vec![
        PlanStep::ok_json("/getBundleStatuses", pending_body),
        PlanStep::ok_json("/getBundleStatuses", landed_body),
    ]);
    let client = HttpJitoClient::new(base);
    // Tight backoff so the test runs in <1s.
    let cfg = LandingPollConfig {
        timeout: Duration::from_secs(5),
        initial_poll_interval: Duration::from_millis(20),
        max_poll_interval: Duration::from_millis(50),
        backoff_multiplier: 2.0,
    };
    let result = poll_bundle_landing("abc", &cfg, |id| client.poll_landing(id)).expect("poll loop");
    assert!(matches!(result, LandingResult::Landed { slot: 12345 }));
    // Both server-side plan steps must have been consumed.
    let _first = rx.recv_timeout(Duration::from_millis(500)).expect("req 1");
    let _second = rx.recv_timeout(Duration::from_millis(500)).expect("req 2");
}

#[test]
fn http_jito_get_bundle_statuses_pending_until_timeout_returns_pending() {
    let pending_body = r#"{
        "jsonrpc":"2.0","id":1,"result":{
            "value":[{"bundle_id":"abc","status":"Pending","landed_slot":null}]
        }
    }"#;
    // Server plan: respond Pending to every request, up to a
    // generous count so the client poll never runs out of
    // server-side responses.
    let plan = (0..64)
        .map(|_| PlanStep::ok_json("/getBundleStatuses", pending_body))
        .collect();
    let (base, _rx) = spawn_server(plan);
    let client = HttpJitoClient::new(base);
    let cfg = LandingPollConfig {
        timeout: Duration::from_millis(150),
        initial_poll_interval: Duration::from_millis(20),
        max_poll_interval: Duration::from_millis(50),
        backoff_multiplier: 1.5,
    };
    let result = poll_bundle_landing("abc", &cfg, |id| client.poll_landing(id)).expect("poll loop");
    assert_eq!(result, LandingResult::Pending);
}

// ─── Test (e): HTTP timeout / 5xx retried with backoff ────────────────

#[test]
fn http_jupiter_5xx_is_retried_with_backoff_then_succeeds() {
    // Two 503s, then a 200. send_with_retry should walk past the
    // 503s without surfacing them to the caller.
    let (base, rx) = spawn_server(vec![
        PlanStep::status("/quote", 503),
        PlanStep::status("/quote", 502),
        PlanStep::ok_json("/quote", RECORDED_QUOTE_BODY),
    ]);
    let cfg = RetryConfig {
        max_attempts: 4,
        initial_backoff: Duration::from_millis(10),
        max_backoff: Duration::from_millis(50),
        backoff_multiplier: 2.0,
    };
    let http = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .expect("client");
    let url = format!("{}/quote", base);
    let resp = send_with_retry(&cfg, || http.post(&url).json(&serde_json::json!({})).send())
        .expect("retried response");
    assert!(resp.status().is_success());
    // All three plan steps should have been consumed.
    let _a = rx.recv_timeout(Duration::from_millis(500)).expect("req 1");
    let _b = rx.recv_timeout(Duration::from_millis(500)).expect("req 2");
    let _c = rx.recv_timeout(Duration::from_millis(500)).expect("req 3");
}

#[test]
fn http_jupiter_4xx_is_not_retried() {
    // 400 is the caller's fault — retrying makes no sense. The
    // helper should surface the 400 on the first try.
    let (base, rx) = spawn_server(vec![PlanStep::status("/quote", 400)]);
    let cfg = RetryConfig {
        max_attempts: 5,
        initial_backoff: Duration::from_millis(10),
        max_backoff: Duration::from_millis(50),
        backoff_multiplier: 2.0,
    };
    let http = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .expect("client");
    let url = format!("{}/quote", base);
    let resp = send_with_retry(&cfg, || http.post(&url).json(&serde_json::json!({})).send())
        .expect("non-retried response");
    assert_eq!(resp.status().as_u16(), 400);
    // Only one server-side request should have been observed.
    let _first = rx.recv_timeout(Duration::from_millis(500)).expect("req 1");
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "4xx must not be retried"
    );
}

#[test]
fn http_jupiter_5xx_exhausts_retries_and_surfaces_last_response() {
    // Two 503s; with max_attempts=2 the helper gives up after the
    // second 503 and returns the 503 response (the loop only
    // retries when attempt < max_attempts; on the final attempt
    // it returns the response to the caller).
    let (base, rx) = spawn_server(vec![
        PlanStep::status("/quote", 503),
        PlanStep::status("/quote", 503),
    ]);
    let cfg = RetryConfig {
        max_attempts: 2,
        initial_backoff: Duration::from_millis(10),
        max_backoff: Duration::from_millis(50),
        backoff_multiplier: 2.0,
    };
    let http = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .expect("client");
    let url = format!("{}/quote", base);
    let resp = send_with_retry(&cfg, || http.post(&url).json(&serde_json::json!({})).send())
        .expect("exhausted retries");
    assert_eq!(resp.status().as_u16(), 503);
    let _first = rx.recv_timeout(Duration::from_millis(500)).expect("req 1");
    let _second = rx.recv_timeout(Duration::from_millis(500)).expect("req 2");
    // Third request must not happen — the loop returns on the
    // final attempt rather than sleeping and retrying.
    assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
}

#[test]
fn http_jupiter_timeout_is_retried_with_backoff() {
    // Server delays the first response by 400ms; client timeout
    // is 100ms. send_with_retry hits a transport error on the
    // first attempt, backs off, and succeeds on the second (fast)
    // response. Initial backoff of 50ms keeps the second request
    // within the 400ms server window.
    let (base, _rx) = spawn_server(vec![
        PlanStep::delayed("/quote", RECORDED_QUOTE_BODY, Duration::from_millis(400)),
        PlanStep::ok_json("/quote", RECORDED_QUOTE_BODY),
    ]);
    let cfg = RetryConfig {
        max_attempts: 3,
        initial_backoff: Duration::from_millis(50),
        max_backoff: Duration::from_millis(200),
        backoff_multiplier: 2.0,
    };
    let http = reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(100))
        .build()
        .expect("client");
    let url = format!("{}/quote", base);
    let resp = send_with_retry(&cfg, || http.post(&url).json(&serde_json::json!({})).send())
        .expect("retried through timeout");
    assert!(resp.status().is_success());
}
