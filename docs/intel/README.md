# Intel — shared findings

This is the durable artifact the team reads. The Intel Manager (this agent) is
the single point of contact for the **external landscape** (prior-art scans via
the Prior-Art Analyst) and the **internal narrative** (project history,
vision/roadmap, decision log via the Project Archivist).

This file is intentionally short. It points at two things:

1. The **noise ledger** — repos / sources the team has decided to ignore so
   future sweeps do not re-litigate them.
2. The **daily briefs** — one dated section per day, with the three things the
   CEO needs to see that day.

## Sections the daily brief populates

Every dated brief under [`daily-briefs/`](./daily-briefs/) covers these four:

1. **One signal worth acting on** — top external finding from the Prior-Art
   Analyst. Either a credible pattern we should borrow, or a credible risk we
   should defend against. Source + credibility verdict required.
2. **One piece of stale context the team is acting on** — from the Project
   Archivist. Something in our own docs, runbook, or roadmap that no longer
   matches the code or the world.
3. **One open question** — anything the team should be asking that nobody is.
4. **Noise ledger delta** — templated/inauthentic repos flagged since
   yesterday. These get promoted to the permanent noise ledger below on
   promotion by the Intel Manager.

## Noise ledger

Repos and sources that have been triaged and dismissed. Don't re-scan unless
asked.

| Source | Verdict | Reason | Decided |
|--------|---------|--------|---------|
| _(empty)_ | — | — | — |

## Routing rules

When a finding lands here, the Intel Manager routes it — never rewrites it:

- Architectural pattern / internal crate concern → CTO
- Strategy / backtest / parameter change → Quant
- Operator-facing UX / runbook text → Product (via CTO/ProductManager)
- Capital, custody, signing, kill-switch → CEO

The Intel Manager does **not** ship code, write specs, or run wallets. Read-only
research only.

## See also

- [`daily-briefs/`](./daily-briefs/) — one dated brief per day
- `damascus_laundry/agents/intel-manager/AGENTS.md` (in Paperclip) — role
  charter
- `crates/`, `docs/architecture.md`, `docs/runbook.md` — the system this brief
  is about
