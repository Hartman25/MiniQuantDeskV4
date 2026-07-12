# PAPER-PNL-OFFMARKET-01C — Route Readback / Test Proof

Patch group: `PAPER-PNL-OFFMARKET-COMPLETION-01-COMBINED`, Phase C.
Read-only DB queries via `docker exec mqk-paper-postgres psql`, plus the
Phase B DB-backed scenario tests, as the proof standard. No DB mutation.
No order submitted. No daemon started or restarted. No code change in this
phase (Phase B already landed the code + test commit).

## 1. Did route tests prove `?timeframe=5m` resolves marks/P&L?

Yes. `scenario_paper_pnl_operator_visibility_01.rs` PPV-11 (committed in
Phase B, commit `6c3f976d`) seeded a single completed `5m` bar
(`close_micros=314_860_000`) for a synthetic position `qty=3
avg_price=314.81` — the exact shape of the real proof-02 AAPL position —
and asserted, via a real in-process `axum::Router` call
(`tower::ServiceExt::oneshot`, no network):

- `GET /api/v1/portfolio/positions?timeframe=5m` →
  `pnl_truth_state="active"`, `mark_price=314.86`,
  `mark_source="md_bars:5m:close"`, `unrealized_pnl≈0.15`.
- `GET /api/v1/portfolio/summary?timeframe=5m` →
  `pnl_truth_state="active"`, `unrealized_pnl≈0.15`.

This test ran for real against the local `mqk-paper-postgres` DB (not
skipped — `MQK_DATABASE_URL` in this environment points at
`localhost:5440/miniquantdesk_paper`), and passed:

```text
running 13 tests
test ppv11_timeframe_5m_resolves_mark_and_pnl_when_only_5m_bar_exists ... ok
...
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

PPV-10 additionally proved the no-query default still resolves `"1D"`
unchanged, and PPV-12 proved the same symbol's default `1D` query still
reports `mark_unavailable` when only a `5m` bar exists — the default did
not silently drift.

## 2. Did DB readback confirm AAPL `5m` marks exist and `1D` rows do not?

Yes, re-confirmed independently in this phase via read-only
`docker exec mqk-paper-postgres psql`:

```sql
select distinct timeframe, count(*) from md_bars where symbol='AAPL' group by timeframe;
--  timeframe | count
-- -----------+-------
--  5m        |  6111
```

Zero rows at `timeframe='1D'`. Latest completed `5m` bar (unchanged since
the prior patch group's Phase D readback):

```text
symbol=AAPL timeframe=5m end_ts=1783707900 (2026-07-10T18:25:00Z)
close_micros=314860000 ($314.86) is_complete=true
```

`runs` readback also re-confirmed proof-02's run is still present and
unchanged:

```text
run_id=15cf4309-210b-5406-8ed8-46377e093195 status=STOPPED
started_at_utc=2026-07-10 18:31:01.324149+00
stopped_at_utc=2026-07-10 18:49:18.481407+00
```

`information_schema.columns` for `md_bars`/`runs`/`oms_outbox`/`oms_inbox`
matched the column names Phase B's code already assumed — no surprises.

## 3. Was live patched-route readback performed?

No.

## 4. If not, why not?

No daemon process is currently running on this machine (`curl
http://127.0.0.1:8899/...` connection refused, no listener on port 8899).
Starting a fresh daemon process is starting new daemon infrastructure,
which this off-market prompt does not authorize ("Do not run the
autonomous smoke script. Do not arm execution." and the Phase C
instructions: "Do not start autonomous runtime... If the daemon predates
the patch, do not restart it unless the operator explicitly authorizes").
Since there is no already-running patched-binary daemon to query
read-only, and starting one was not requested or authorized, this phase
relies on the Phase B DB-backed route tests (§1) as the proof standard —
consistent with `CLAUDE.md`'s "Scenario tests are the proof standard"
instruction.

## 5. What would the real AAPL proof-02 position compute from the latest `5m` mark, if calculable from DB readback?

Hand-calculated from the real DB values in §2 (same arithmetic
`unrealized_pnl_micros` implements, already proven correct by PPV-05/06
against seeded bars, and now also proven correct against this exact
`qty=3 avg_price=314.81` shape by PPV-11):

```text
mark_price_micros    = 314_860_000   ($314.86)
avg_price_micros     = 314_810_000   ($314.81)
qty                  = 3
unrealized_pnl_micros = (314_860_000 - 314_810_000) * 3 = 150_000
unrealized_pnl        = $0.15
```

If a daemon running the Phase B binary were started against this same
paper DB with `?timeframe=5m`, `GET /api/v1/portfolio/positions` would be
expected to report exactly this for the real `AAPL` position — but that
live call was not made in this phase (§3/§4).

## 6. Did any DB mutation occur?

No. Every query in this phase is a `select`. No `insert`/`update`/`delete`
was executed against `miniquantdesk_paper` in this phase (the Phase B test
suite's own `insert_test_bar`/`delete_test_bars` helpers operate on
synthetic non-`AAPL` symbols like `ZZPPV11FIV`, are self-cleaning within
each test, and are outside this Phase C session — this phase performed
read-only inspection only).

## 7. Did any order occur?

No. No broker call, no order submit, no OMS write of any kind.

## 8. Did any daemon restart occur?

No. No daemon was running before this phase and none was started or
restarted during it.

## 9. Conclusion

- Route/test proof: **complete** — PPV-10 through PPV-14 (Phase B) prove
  default-`1D`-unchanged and `?timeframe=5m`-resolves-mark-and-P&L against
  a DB-backed, in-process route call using the real proof-02 position
  shape.
- Real DB state: **re-confirmed** — AAPL has 6111 completed `5m` rows,
  zero `1D` rows, latest close `$314.86`.
- Live patched-daemon route readback: **not performed** — no daemon
  process is currently running, and starting one is out of this
  off-market phase's authorized scope.
- No DB mutation, no order, no daemon restart, no code change in this
  phase.
