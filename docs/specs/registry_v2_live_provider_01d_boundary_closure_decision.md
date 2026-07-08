# REGISTRY-V2-LIVE-PROVIDER-01D — Boundary Closure Decision

Patch ID: `REGISTRY-V2-LIVE-PROVIDER-01D-CLOSURE-AND-ROADMAP-RECONCILE-01`

Decision-only. No code changed by this patch. Written after `01A` (current
non-equity provider audit), `01B` (first proof boundary decision), and `01C`
(no-network preflight guard) to decide whether the *boundary decision* for
`ASSET-CORE-01H` prerequisite #4 is closed, and to reconcile the roadmap
accordingly. This patch does not close prerequisite #4 itself.

---

## 1. Is prerequisite #4 closed?

**No.** Prerequisite #4 reads:

> At least one non-equity market-data provider live-network-verified
> (not just fixture/CSV-proven) end-to-end into `md_bars`.

No live network call was made in this patch or any of its three prior
phases. No bars reached `md_bars` from a live Kraken call. Prerequisite #4
remains **open** until a future, separately-authorized session runs the
exact command named in `01B` §4 with the exact operator authorization named
in `01B` §7, and produces evidence matching `01B` §9.

## 2. Is the boundary decision for prerequisite #4 closed?

**Yes**, given `01A`-`01C` passed their validators (§confirmed below):

- `01A` grounds the decision in current, verified repo evidence — Kraken is
  the only candidate with a real tested adapter, an existing DB-writing CLI
  command, no credential requirement, and an already-fail-closed network
  gate.
- `01B` names the exact provider, symbols, timeframe, allowed command,
  allowed DB target, allowed evidence location, required operator
  authorization phrase, forbidden command shapes, required/forbidden proof
  fields, a post-proof no-cutover check, and explicitly leaves prerequisite
  #5 untouched.
- `01C` adds a purely local, no-network, no-DB preflight guard that
  mechanically confirms `01B` contains every one of those required elements
  (including the exact authorization phrase, byte-for-byte) and that no
  forbidden source/config file or generated-evidence file was touched by
  this bundle's own diff.

## 3. Which provider/symbol/timeframe were selected for the future proof?

**Kraken**, symbols **`BTC/USD`** and/or **`ETH/USD`**, timeframe **`1D`**
— per `01B` §1-§3, unchanged here.

## 4. What authorization phrase is required next?

Verbatim, from `01B` §7:

```text
I explicitly authorize ONE bounded Kraken public OHLC live-network proof for BTC/USD and ETH/USD into an isolated test/proof database only. Do not enable trading, do not use credentials, do not touch paper/live broker routing, and stop after evidence.
```

## 5. What future proof command is allowed only after that phrase?

```text
mqk md kraken-ohlc-ingest --registry <registry-v2 path> --symbol <BTC/USD|ETH/USD> --timeframe 1D [--output-dir <evidence dir>]
```

run with `MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1` set, no `--input-file`, and
`MQK_DATABASE_URL` pointed at an isolated proof/test database — per `01B`
§4-§5. No other command (`kraken-ohlc-sync`, scheduler scripts, or any
ad-hoc HTTP call) is authorized.

## 6. What future evidence must be present?

The `--output-dir` evidence JSON (or daemon-surfaced equivalent) must show
`mode="network_smoke"`, `network_call_made=true`, `db_write=true`,
`md_bars_write=true`, `bars_completed>0`, truthful `provider_id="kraken"`
metadata, and a confirmed isolated-proof-DB target — per `01B` §9.

## 7. What must remain false/unchanged?

`config/providers/providers.json`'s `kraken.enabled`, both crypto
registry-v2 fixture rows' `enabled`/`paper_trading_enabled`/
`live_trading_enabled`, and the absence of any registered Windows Scheduled
Task must all remain exactly as they are today — per `01B` §10.

## 8. What remains before production cutover?

Per `ASSET-CORE-01H` §5 and `roadmap_completion_reconcile_01.md` §2:

1. ~~`BACKTEST-MULTIPLIER-MARGIN-01` closed~~ — satisfied.
2. ~~Symbol/`instrument_id` translation layer~~ — satisfied
   (`REGISTRY-V2-TRANSLATION-01A`-`01D`).
3. ~~Gate 0 / broker-submit routing-guard parity~~ — satisfied
   (`REGISTRY-V2-GATE-PARITY-01A`-`01D`).
4. Live-network non-equity provider proof — **boundary now decided by this
   bundle; the proof itself remains open**, pending explicit operator
   authorization (§4) and the exact command in §5.
5. Explicit operator enablement decision for a named non-equity instrument
   — still open, and must not precede #4's actual live proof.

`REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01` remains blocked on
prerequisites #4 (proof itself, not its boundary) and #5, and is **not**
recommended next.

## 9. What next prompt is recommended?

Only if the operator explicitly provides the exact phrase in §4:
**`REGISTRY-V2-KRAKEN-LIVE-PROVIDER-PROOF-01`** — a bounded, single-purpose
patch that runs exactly the command in §5 against an isolated proof
database, captures evidence per §6, and reconciles the roadmap once the
live proof (not just its boundary) is settled. Otherwise, stop and ask the
operator for that explicit authorization before any live network proof is
attempted.

---

## Closure decision

```text
REGISTRY-V2-LIVE-PROVIDER-PROOF-BOUNDARY-DECISION-01 is CLOSED_LOCAL.
Prerequisite #4 itself remains OPEN until an explicitly authorized
live-network proof is run and evidence proves completed non-equity bars
reached md_bars through the guarded path (mqk md kraken-ohlc-ingest).
No live network call occurred in this boundary patch.
No DB access or mutation occurred.
No trading was enabled.
```

**Recommended next patch:** `REGISTRY-V2-KRAKEN-LIVE-PROVIDER-PROOF-01`,
**only** once the operator has given the exact authorization phrase in §4.
