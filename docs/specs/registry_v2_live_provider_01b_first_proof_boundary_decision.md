# REGISTRY-V2-LIVE-PROVIDER-01B — First Proof Boundary Decision

Patch ID: `REGISTRY-V2-LIVE-PROVIDER-01B-FIRST-PROOF-BOUNDARY-DECISION-01`

Decision-only. No code, no network call, no DB access. Written after `01A`'s
audit (`docs/specs/registry_v2_live_provider_01a_current_non_equity_provider_audit.md`)
to name the exact boundary a future, separately-authorized session must
respect before making any live network call toward `ASSET-CORE-01H`
prerequisite #4.

---

## 1. Which provider is selected for the first future live proof?

**Kraken** (`config/providers/providers.json`'s `kraken` entry;
`core-rs/crates/mqk-md/src/providers/kraken.rs`). Rationale is fully
established by `01A` §3: it is the only candidate with a real, tested
`HistoricalProvider` adapter and an existing DB-writing CLI command, it
requires no credentials, and its network call is already fail-closed
behind an explicit operator opt-in.

## 2. Which symbol(s) are selected?

**`BTC/USD` and `ETH/USD`**, proven one at a time (one CLI invocation per
symbol) — the only two symbols the Kraken adapter and the disabled
registry-v2 fixture (`config/instruments/instruments_v2.crypto_local_marks.example.json`)
support (`01A` §4). Proving prerequisite #4 requires only one of the two to
succeed; proving both is stronger evidence but not required to close it.

## 3. Which timeframe is selected?

**`1D`** — the only timeframe `KrakenHistoricalProvider` and
`md_kraken_ohlc_ingest` accept; any other value is rejected before any
network call is attempted (`01A` §5).

## 4. Which command is allowed in the future?

Exactly one command family, run at most once per symbol:

```text
mqk md kraken-ohlc-ingest --registry <registry-v2 path> --symbol <BTC/USD|ETH/USD> --timeframe 1D [--output-dir <evidence dir>]
```

run with `MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1` explicitly set and **no**
`--input-file` argument (the only configuration that makes this command
perform a network call, per `01A` §6). No other CLI command, script, or
ad-hoc HTTP call is authorized by this decision — not `kraken-ohlc-sync`
(which additionally requires `MQK_ALLOW_KRAKEN_SCHEDULED_SYNC` and is a
recurring-sync path, not a one-shot proof), not `kraken-ohlc-dry-run`
(which does not write to `md_bars` and therefore cannot satisfy
prerequisite #4's "end-to-end into `md_bars`" requirement), and not
`Run-KrakenOhlcSync.ps1`/`Register-KrakenOhlcSyncTask.ps1` (scheduler
wrappers, explicitly forbidden — see §4 hard rule below).

## 5. Which DB target is allowed in the future?

**An isolated proof/test database only, never the paper or live
database.** `kraken-ohlc-ingest` connects via `mqk_db::connect_from_env()`,
i.e. whatever `MQK_DATABASE_URL` resolves to at invocation time — this
command does not choose or validate its DB target itself. The future
authorized session **must**:

- Set `MQK_DATABASE_URL` to point at a database used for this proof only
  (e.g. an existing local/test Postgres instance already used for
  DB-backed scenario tests, or a fresh, disposable local Postgres
  instance) before running the command.
- Never point `MQK_DATABASE_URL` at the paper-trading database
  (`postgres://.../miniquantdesk_paper` per prior session memory) or any
  live/production database when performing this specific proof.
- Confirm the target DB's identity (e.g. via `\conninfo` or an explicit
  connection-string echo, redacting credentials) in the evidence captured
  from that run, so a reviewer can confirm the write landed in the
  isolated proof DB and not a shared one.

## 6. Which evidence directory/file pattern is allowed?

`kraken-ohlc-ingest --output-dir` writes
`kraken_ohlc_ingest_<epoch>.json` into the given directory (`01A` §6). The
future authorized session must pass a dedicated, clearly-labeled
`--output-dir` (e.g. `exports/live_provider_proof/` or an equivalent
gitignored evidence directory) — not `smoke_logs/` (already used for
unrelated smoke evidence) and not any tracked config/source directory.
This decision does not authorize staging that evidence file into git; per
this session's hard safety rules, no generated evidence, export, or raw
provider response is staged by any phase of this patch, and the same
applies to a future live-proof session unless a separate, explicit
operator decision says otherwise.

## 7. What exact operator authorization text is required before any future network call?

The future session must receive this exact phrase, verbatim, from the
operator before making any live network call:

```text
I explicitly authorize ONE bounded Kraken public OHLC live-network proof for BTC/USD and ETH/USD into an isolated test/proof database only. Do not enable trading, do not use credentials, do not touch paper/live broker routing, and stop after evidence.
```

Absent this exact phrase (or an operator-provided equivalent that a human
reviewer confirms carries the same scope and restrictions), no future
session may set `MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1` and omit `--input-file`
when invoking `kraken-ohlc-ingest`.

## 8. What exact command is forbidden unless the authorization text is present?

Any invocation of `mqk md kraken-ohlc-ingest` (or `kraken-ohlc-dry-run`,
`kraken-ohlc-sync`) with `MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1` (and, for
`kraken-ohlc-sync`, `MQK_ALLOW_KRAKEN_SCHEDULED_SYNC=1`) set and no
`--input-file` provided is forbidden until the exact §7 phrase has been
given. This is enforced today at the code level by the existing fail-closed
gate in `core-rs/crates/mqk-cli/src/commands/md.rs`
(`ENV_ALLOW_KRAKEN_NETWORK_SMOKE`) — this decision adds an operator-process
gate on top of that code-level gate, not a replacement for it.

## 9. What proof fields must be true to close prerequisite #4?

A future evidence file (or daemon-surfaced equivalent) must show, for at
least one of `BTC/USD`/`ETH/USD`:

- `mode: "network_smoke"` (not `"input_file"`).
- `network_call_made: true`.
- `db_write: true`.
- `md_bars_write: true`.
- `bars_completed > 0` (at least one real, non-fixture completed bar
  reached `md_bars`).
- `provider_id: "kraken"` / `provider_source: "kraken"` (truthful
  provenance, not `"unknown"` or a fixture label).
- A confirmed DB-target identity showing the write landed in the isolated
  proof database named in §5, not paper/live.
- A `forming_candle_excluded` field consistent with the always-true
  structural guarantee already tested in `01A`/the Kraken adapter's own
  test suite (the not-yet-committed candle is never written).

## 10. What proof fields must remain false to avoid implying trading readiness?

The same evidence, and the repo state around it, must simultaneously show:

- `config/providers/providers.json`'s `kraken.enabled` remains `false`.
- Both registry-v2 fixture rows' `enabled`, `paper_trading_enabled`, and
  `live_trading_enabled` remain `false`.
- No Windows Scheduled Task was registered
  (`Register-KrakenOhlcSyncTask.ps1` not run).
- No recurring `kraken-ohlc-sync` invocation occurred — only the one-shot
  `kraken-ohlc-ingest` proof.
- No `mqk-runtime`/`mqk-execution`/`mqk-risk`/`mqk-broker-alpaca` file was
  touched or began consuming the newly-written `md_bars` rows.

## 11. How to prove no production cutover occurred after the future live proof

A follow-up closure check (analogous to this bundle's own Phase D) must
confirm, via direct repo/DB inspection after the live proof runs:

- `git diff --name-only` against the pre-proof commit touches no file
  outside `docs/`, `scripts/guards/`, and evidence/export directories.
- The proof database (§5) is distinct from the paper/live database by
  connection string, and the paper/live database's `md_bars` table shows
  no new Kraken-sourced rows from the proof timestamp.
- `config/providers/providers.json` and both crypto registry-v2 fixture
  rows are byte-identical to their pre-proof state (still disabled).
- No scheduled task exists (`schtasks /query` or equivalent shows no
  Kraken sync task registered).

## 12. What remains for prerequisite #5

Prerequisite #5 (`ASSET-CORE-01H` §5 item 5: "An explicit operator decision
to enable `enabled=true` for a specific, named non-equity instrument —
never inferred from schema presence alone") remains entirely untouched by
this decision and must not be attempted before prerequisite #4's live proof
actually runs and its evidence is reviewed. Enabling an instrument before
its data source is live-network-proven would invert the checklist's own
ordering, as `roadmap_completion_reconcile_01.md` §3 already states.

---

## Decision

```text
Selected first live proof provider: Kraken public OHLC/OHLCV for disabled
BTC/USD and ETH/USD registry-v2 rows, via `mqk md kraken-ohlc-ingest`
(--timeframe 1D), gated behind MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1.

Future proof target: an isolated proof/test database only, set via
MQK_DATABASE_URL, never the paper or live database.

Future proof goal: one explicitly authorized public market-data call path
that writes completed non-equity bars into md_bars through the existing
guarded CLI path and emits evidence with network_call_made=true,
db_write=true, md_bars_write=true, bars_completed>0.

Not allowed yet: scheduled sync, production registry-v2 cutover, crypto
trading enablement, broker routing, strategy execution, risk changes, or
provider config enablement (kraken.enabled must stay false throughout).

Required operator authorization phrase (verbatim, before any future
network call):

  I explicitly authorize ONE bounded Kraken public OHLC live-network proof
  for BTC/USD and ETH/USD into an isolated test/proof database only. Do
  not enable trading, do not use credentials, do not touch paper/live
  broker routing, and stop after evidence.
```

This decision does not run the proof. No network call, DB access, or
config change occurred while writing it.
