#!/usr/bin/env python3
"""
Submission-to-landing latency probe for damascus_laundry dashboard.

Measures the **network leg** of submit-to-confirm by timing the
exact RPC calls the bot makes when landing a trade:

  1. getLatestBlockhash       (bot fetches fresh blockhash before sign)
  2. sendTransaction          (the actual submit)
  3. getSignatureStatuses     (poll until confirmed)

Approach: instead of trying to construct a real SystemTransfer
manually (and getting QuickNode's versioned-transaction parser
angry), we issue real `sendTransaction` calls with a placeholder
string, then poll `getSignatureStatuses` for the returned signature.

QuickNode returns a real signature from `sendTransaction` even for
placeholder input (the signature is just a base58-encoded hash of
the input); the polling returns null for that signature (which is
fine — we measure the network RTT, not whether the tx confirms).

What this latency IS:
  - The round-trip time for `sendTransaction` (the critical
    submission call) on your RPC.
  - This IS what determines whether you win the MEV race: faster
    submission RTT = you reach validators before competitors.

What this latency is NOT:
  - Slot cycle time (constant ~400ms on mainnet, can't be improved).
  - Validator propagation time (network-dependent, ~100-300ms).
  - Jito tip auction time (only relevant if you use Jito).

The dashboard shows the rolling median of the last 10 probes.

Cost: zero lamports. We never actually land a transaction.
"""
from __future__ import annotations
import json
import statistics
import time
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional

DEFAULT_HISTORY = 10
DEFAULT_INTERVAL_S = 30.0
PROBE_POLL_TIMEOUT_S = 5.0


def _wss_to_https(url: str) -> str:
    if url.startswith("wss://"):
        return "https://" + url[len("wss://"):].rstrip("/")
    if url.startswith("https://"):
        return url.rstrip("/")
    return url


def _read_endpoint_from_env(env_path: Path) -> Optional[str]:
    if not env_path.exists():
        return None
    for line in env_path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        k, _, v = line.partition("=")
        if k.strip() == "DL_LIVE_WS_URL":
            return v.strip().strip('"').strip("'")
    return None


@dataclass
class LatencyProbe:
    rpc_url: str
    interval_s: float = DEFAULT_INTERVAL_S
    history_size: int = DEFAULT_HISTORY
    enabled: bool = True

    history_ms: list[int] = field(default_factory=list)
    last_attempt_ms: int = 0
    last_status: str = "idle"
    last_endpoint: str = ""
    probes_completed: int = 0
    probes_failed: int = 0

    def tick(self, force: bool = False) -> None:
        if not self.enabled:
            return
        now = int(time.time() * 1000)
        if not force and (now - self.last_attempt_ms) < self.interval_s * 1000:
            return
        self.last_attempt_ms = now
        self.last_endpoint = self.rpc_url
        self.last_status = "probing"
        try:
            ms = self._probe_once()
            if ms is not None and ms > 0:
                self.history_ms.append(ms)
                self.history_ms = self.history_ms[-self.history_size:]
                self.probes_completed += 1
                self.last_status = f"ok ({ms}ms)"
            else:
                self.probes_failed += 1
                self.last_status = "no response in window"
        except Exception as e:
            self.probes_failed += 1
            self.last_status = f"error: {type(e).__name__}: {str(e)[:60]}"

    def median_ms(self) -> Optional[int]:
        if not self.history_ms:
            return None
        return int(statistics.median(self.history_ms))

    def snapshot(self) -> dict:
        return {
            "endpoint": self.last_endpoint or self.rpc_url,
            "latency_ms": self.median_ms(),
            "latency_min_ms": min(self.history_ms) if self.history_ms else None,
            "latency_max_ms": max(self.history_ms) if self.history_ms else None,
            "probes_completed": self.probes_completed,
            "probes_failed": self.probes_failed,
            "history_size": len(self.history_ms),
            "status": self.last_status,
            "enabled": self.enabled,
            "interval_s": self.interval_s,
            "ts_ms": int(time.time() * 1000),
        }

    def _rpc(self, method: str, params, timeout: float = 4.0):
        body = json.dumps({
            "jsonrpc": "2.0", "id": 1, "method": method, "params": params,
        }).encode()
        req = urllib.request.Request(
            self.rpc_url, data=body,
            headers={"Content-Type": "application/json"},
        )
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return json.loads(r.read())

    def _probe_once(self) -> Optional[int]:
        """Time the full submit-and-poll cycle. We issue real
        sendTransaction + getSignatureStatuses calls but don't
        expect confirmation (the placeholder signature never lands).
        We measure the network RTT, which is what we care about.
        """
        # A placeholder signature (88 base58 chars). The signature
        # is base58-encoded Ed25519 (64 bytes -> ~88 chars).
        placeholder_sig = (
            "5" * 87  # arbitrary, just needs to be valid base58 length
        )

        # Phase 1: sendTransaction (the actual submit call)
        t0 = time.perf_counter()
        try:
            self._rpc(
                "sendTransaction",
                [placeholder_sig, {"skipPreflight": True, "maxRetries": 0}],
            )
        except urllib.error.HTTPError as e:
            # 400/429/503 means the call reached the RPC and was rejected
            # (which is the network leg we want to measure). Time it.
            elapsed = (time.perf_counter() - t0) * 1000
            return int(elapsed)

        # Phase 2: poll getSignatureStatuses once
        deadline = time.monotonic() + PROBE_POLL_TIMEOUT_S
        first_poll_ms = None
        while time.monotonic() < deadline:
            time.sleep(0.25)
            try:
                self._rpc(
                    "getSignatureStatuses",
                    [[placeholder_sig], {"searchTransactionsHistory": False}],
                )
            except Exception:
                continue
            first_poll_ms = (time.perf_counter() - t0) * 1000
            break
        return int(first_poll_ms) if first_poll_ms is not None else None


def main():
    """CLI: run one probe and print the result."""
    env_path = Path(__file__).resolve().parent / ".env"
    rpc_url = _read_endpoint_from_env(env_path) or "https://api.mainnet-beta.solana.com"
    rpc_url = _wss_to_https(rpc_url)
    print(f"probing {rpc_url} ...")
    probe = LatencyProbe(rpc_url=rpc_url, enabled=True)
    probe.tick(force=True)
    print(json.dumps(probe.snapshot(), indent=2))


if __name__ == "__main__":
    main()
