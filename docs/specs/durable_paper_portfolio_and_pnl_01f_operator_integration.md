# DURABLE-PAPER-PORTFOLIO-AND-PNL-01F — GUI, Runbook, and Evidence Integration

Patch ID: `DURABLE-PAPER-PORTFOLIO-AND-PNL-01F-OPERATOR-INTEGRATION`
Projects B4-B/B4-C/B4-D/B4-E's durable truth onto the operator Portfolio
screen, the autonomous paper ops runbook, and the Bundle 3 soak-evidence
tooling. No mutation control added anywhere in this patch.

## GUI

`PortfolioScreen.tsx` gained a `DurablePortfolioSection` (three panels:
"Durable Portfolio (restart-surviving)", "Durable positions", "Recent
durable snapshots"), fed by three new `SystemModel` fields
(`durablePortfolioSummary`, `durablePortfolioPositions`,
`durablePortfolioSnapshots`) fetched from the three B4-E routes, folded
into the existing tracked-probes `Promise.all` batch in `api.ts` (same
pattern as every other durable-run surface in this file, e.g.
`autonomousDailyOperation`).

**The durable section renders unconditionally** — it does not sit behind
the existing in-memory panel's hard-close gate
(`panelTruthRenderState(model, "portfolio")`). This is deliberate, not an
oversight: durable truth is exactly what remains visible when in-memory
broker-snapshot truth is degraded or absent (most notably right after a
daemon restart, before a fresh broker snapshot has been fetched), so gating
it behind the same check would hide it precisely when it is most useful.
Verified live in the browser preview: with the daemon disconnected (the
in-memory panel showing its "Live truth is currently unreachable" hard
block), the durable section still rendered independently, correctly
showing "Unavailable" for every field alongside an explicit
`snapshot_truth_state: db_unavailable` notice — proving the two truths are
never collapsed into one.

**Null-vs-zero rendering.** `formatDurableMoney`/`formatDurableCount`
render `null` as the literal word "Unavailable" (styled via a new
`.val-unavailable` CSS token — distinct from the existing `formatMoney`'s
"—" glyph, which the broker-snapshot panels above use for their own,
separate pre-existing convention) and a real `0` as `$0.00` — proven by
`durable_summary_incomplete_epoch_blocks_realized_pnl_but_shows_position`
in B4-E's test suite, whose scenario the GUI now visually reflects:
`realized_pnl: null` renders "Unavailable" while `account_equity` (a real
number from the same response) renders normally.

**No mutation control.** This section contains no capture/retry/order/arm/
flatten button or form of any kind — display only, matching the mission's
explicit requirement. `SystemModel`'s three new fields were also added to
`useOperatorModel.ts`/`useSystemModel.ts`'s fallback model constructors and
`mockData.ts`'s mock model (all default to an explicit `"unavailable"`
truth state with every data field `null`, never a fabricated zero).

## Runbook (`docs/runbooks/autonomous_paper_ops.md`)

New §16d documents the three routes, how to distinguish durable from
in-memory truth, the `snapshot_truth_state`/`accounting_truth_state`
vocabulary, and what `fill_history_incomplete` means (a position adopted
before this system's fill history began — never silently fixed by
inventing an opening fill). `GET /api/v1/execution/paper-lifecycle`'s two
new `overall_lifecycle_state` values are documented alongside it. §17's
before-session checklist, §21's end-of-day evidence list, and §22 (new
§22a) cross-reference it; §22 gained a restart-distinctions table row for
an already-adopted broker position with incomplete fill history. §23
gained an explicit prohibition against manually editing the three new
tables or fabricating a synthetic opening fill to force
`accounting_epoch` to `"complete"`.

## Soak-evidence tooling (Bundle 3, extended)

`capture_autonomous_paper_session_evidence.ps1` fetches the three new
routes through the same `Invoke-DaemonGetOnly` GET-only seam every other
route in the script uses — a failure is recorded as a bounded error record
+ missing-endpoint entry exactly like any other route, never a fabricated
fallback. The three fields are added to both the manifest object and the
SHA-256 `artifact_hashes` candidate list.

`validate_autonomous_paper_session_evidence.ps1` gained: the three fields
in the required-fields presence check ([3]); the three routes in the
missing-endpoints cross-check ([11]); a new check [13] specifically for
durable truth — every present `durable_portfolio_*` object must carry its
own explicit truth-state field(s) (never a bare data blob with no
provenance), and `accounting_epoch`, when present, must be exactly
`"complete"` or `"incomplete"`.

`autonomous_paper_session_manifest.template.json` and
`supervised_session_evidence_checklist.md` were updated to match (the
checklist gained an explicit reminder to check
`durable_portfolio_summary.accounting_truth_state` after each capture).

`test_autonomous_paper_session_evidence.ps1`'s baseline fixture set and
validator-test baseline manifest both gained fully-populated, valid durable
fields (so the existing "happy path" positive test stays green), plus five
new negative cases proving check [13] actually rejects: a missing
`truth_state` on `durable_portfolio_summary`/`_positions`/`_snapshots`, a
missing `accounting_truth_state`, and an `accounting_epoch` value outside
the closed `complete`/`incomplete` vocabulary.

All Bundle 3 GET-only/local-daemon-only/secret-safety/null-count-preservation
guarantees are unchanged — this patch only adds fields, it does not modify
any existing check's behavior.

## Verification

- `npm run build` (tsc typecheck + vite build) in `core-rs/mqk-gui`: clean.
- `npm test` in `core-rs/mqk-gui`: 850/850 pass.
- Live browser verification (dev server via the project's own
  `.claude/launch.json` config, daemon intentionally not running): the
  Portfolio screen's durable section rendered correctly and independently
  of the in-memory panel's hard-close notice, with the exact three new
  network requests (`durable-summary`, `durable-positions`,
  `durable-snapshots?limit=20`) observed firing; zero console errors.
- `scripts/soak/tests/test_autonomous_paper_session_evidence.ps1`: 40/40
  scenarios pass (35 pre-existing + 5 new durable-truth negative cases).

## FINAL-RUN-SCOPING-ACCOUNTING-AND-CLOSURE-REPAIR addendum

Patch ID: `DURABLE-PAPER-PORTFOLIO-AND-PNL-01-FINAL-RUN-SCOPING-ACCOUNTING-AND-CLOSURE-REPAIR`

New module `core-rs/mqk-gui/src/features/system/durablePortfolio.ts` hardens
the GUI runtime contract for all three durable-portfolio routes, closing the
gap where a successful HTTP 200 was trusted at face value
(`result.data as DurablePortfolioSummary`) with no shape or
`truth_state`-vocabulary validation:

- `parseDurablePortfolioSummary` / `parseDurablePortfolioPositions` /
  `parseDurablePortfolioSnapshots` validate every required field and check
  every `truth_state` value against a closed vocabulary
  (`DURABLE_SUMMARY_TRUTH_STATES` etc.); a malformed body or an
  unrecognized `truth_state` fails closed to the same unavailable sentinel
  an unreachable fetch already produced.
- `enforceRunScopeConsistency` rejects a positions response whose `run_id`
  differs from an `active` summary's `run_id` when both claim `"active"` —
  the GUI-side backstop for the same cross-run-contamination class of bug
  Repair A closes on the daemon side.
- `api.ts`'s `fetchOperatorModel` now calls these validators instead of
  casting `durablePortfolioSummaryR.data`/`Positions`/`Snapshots` directly.
- `DurablePortfolioSummary` gained `accounting_source_snapshot_id: string |
  null`; `PortfolioScreen.tsx`'s durable section now displays it under
  "Accounting source snapshot" (read-only, no mutation control added).
- New test file `durablePortfolio.test.ts` (18 tests): valid-response
  acceptance, malformed-body rejection, unrecognized-truth_state rejection,
  null-vs-zero preservation, `fill_history_incomplete`/`query_failed`
  preservation, history-order preservation, and the run-mismatch guard in
  both directions (rejects on mismatch, passes through on match, does not
  fire when either side is not `"active"`).

Verification: `npm test` in `core-rs/mqk-gui`: 866/866 pass (848 pre-existing
+ 18 new). `npm run build`: clean (tsc + vite).
