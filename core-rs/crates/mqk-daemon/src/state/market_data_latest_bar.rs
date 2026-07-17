//! AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-COMPLETED-BAR-DATA-DRIVER: extracted
//! internal seam for polling one provider's latest closed bar and ingesting
//! it into canonical `md_bars`.
//!
//! Before this patch, this logic existed only inlined inside the
//! `POST /api/v1/market-data/feed/poll-once` Axum handler
//! (`routes/ingest.rs::market_data_feed_poll_once`). That handler now calls
//! the two functions in this module per symbol; behavior (status strings,
//! row counts, error wording) is preserved exactly. `execute_scheduler_poll_once`
//! no longer needs to round-trip through JSON serialization to reach this
//! logic in-process — though that refactor is left to the route file itself,
//! this module is what makes it possible.
//!
//! This module makes zero provider calls and zero DB writes on its own
//! initiative — every function here is a pure decision or a single
//! caller-invoked I/O step. No historical sync, no backfill, no ingest-job
//! creation: only the latest-closed-bar path.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use mqk_md::instrument_registry::TrackedInstrument;

/// Why a `(local_symbol, timeframe)` pair was not admitted for polling
/// against a given provider. Mirrors the exact per-symbol skip reasons the
/// manual poll-once route has always produced — extraction preserves
/// wording, not just behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LatestBarRegistryAdmissionRejection {
    InstrumentNotFound,
    InstrumentDisabled,
    ProviderMismatch { configured_provider: String },
    BlankProviderSymbol,
    TimeframeNotAuthorized { configured_timeframes: Vec<String> },
}

impl LatestBarRegistryAdmissionRejection {
    /// Status label matching the manual route's existing
    /// `MarketDataFeedPollSymbolResult.status` values verbatim.
    pub fn status_label(&self) -> &'static str {
        match self {
            Self::InstrumentNotFound => "skipped_instrument_not_in_registry",
            Self::InstrumentDisabled => "skipped_instrument_disabled",
            Self::ProviderMismatch { .. } => "skipped_provider_mismatch",
            Self::BlankProviderSymbol => "skipped_blank_provider_symbol",
            Self::TimeframeNotAuthorized { .. } => "skipped_instrument_timeframe_unsupported",
        }
    }

    /// Error detail matching the manual route's existing per-symbol error
    /// message wording verbatim.
    pub fn detail(
        &self,
        local_symbol: &str,
        requested_provider_id: &str,
        timeframe: mqk_md::Timeframe,
    ) -> String {
        match self {
            Self::InstrumentNotFound => format!(
                "symbol '{local_symbol}' not found in instrument registry; \
                 cannot resolve canonical provider mapping"
            ),
            Self::InstrumentDisabled => {
                format!("instrument '{local_symbol}' is disabled in the instrument registry")
            }
            Self::ProviderMismatch {
                configured_provider,
            } => format!(
                "instrument '{local_symbol}' is configured for provider '{configured_provider}', \
                 not the requested poll provider '{requested_provider_id}'"
            ),
            Self::BlankProviderSymbol => {
                format!("instrument '{local_symbol}' has a blank canonical provider_symbol")
            }
            Self::TimeframeNotAuthorized {
                configured_timeframes,
            } => format!(
                "instrument '{local_symbol}' does not authorize timeframe '{}' (configured: {:?})",
                timeframe.as_str(),
                configured_timeframes
            ),
        }
    }
}

/// `true` iff `instrument.timeframes` (as configured strings) authorizes
/// `timeframe`. Moved here, unchanged, from the manual poll-once route so
/// both callers share one implementation.
pub fn instrument_authorizes_timeframe(
    instrument: &TrackedInstrument,
    timeframe: mqk_md::Timeframe,
) -> bool {
    instrument.timeframes.iter().any(|tf| {
        mqk_md::Timeframe::parse(tf)
            .map(|parsed| parsed == timeframe)
            .unwrap_or(false)
    })
}

/// One already-admitted poll target: a canonical `(local_symbol, timeframe)`
/// pair proven to be enabled, mapped to `provider_id`, and carrying a
/// non-blank canonical `provider_symbol`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLatestBarPollTarget {
    pub provider_id: String,
    pub provider_symbol: String,
    pub local_symbol: String,
    pub timeframe: mqk_md::Timeframe,
}

/// Resolve and admit one local symbol against an already-loaded, validated
/// instrument registry and an already-resolved `provider_id`. Performs zero
/// provider calls and zero DB access — pure registry lookup, reused by both
/// the manual poll-once route and the Phase C autonomous driver so
/// admission policy can never drift between the two callers (C.8: "reuse
/// the canonical registry and provider admission logic already used by
/// historical provider sync, latest-bar poll, daily readiness").
pub fn resolve_latest_bar_poll_target(
    instruments: &[TrackedInstrument],
    provider_id: &str,
    local_symbol: &str,
    timeframe: mqk_md::Timeframe,
) -> Result<ResolvedLatestBarPollTarget, LatestBarRegistryAdmissionRejection> {
    let instrument = instruments
        .iter()
        .find(|i| i.symbol.trim().eq_ignore_ascii_case(local_symbol.trim()))
        .ok_or(LatestBarRegistryAdmissionRejection::InstrumentNotFound)?;

    if !instrument.enabled {
        return Err(LatestBarRegistryAdmissionRejection::InstrumentDisabled);
    }
    if !instrument
        .provider
        .trim()
        .eq_ignore_ascii_case(provider_id.trim())
    {
        return Err(LatestBarRegistryAdmissionRejection::ProviderMismatch {
            configured_provider: instrument.provider.clone(),
        });
    }
    let provider_symbol = instrument.provider_symbol.trim();
    if provider_symbol.is_empty() {
        return Err(LatestBarRegistryAdmissionRejection::BlankProviderSymbol);
    }
    if !instrument_authorizes_timeframe(instrument, timeframe) {
        return Err(
            LatestBarRegistryAdmissionRejection::TimeframeNotAuthorized {
                configured_timeframes: instrument.timeframes.clone(),
            },
        );
    }

    Ok(ResolvedLatestBarPollTarget {
        provider_id: provider_id.to_string(),
        provider_symbol: provider_symbol.to_string(),
        local_symbol: instrument.symbol.clone(),
        timeframe,
    })
}

/// REPAIR 4 (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-POLL-REEVALUATE-AND-PROVIDER-CLOSURE-01):
/// an optional exact-identity constraint the autonomous driver attaches to a
/// poll request so the shared ingest seam can distinguish "the provider
/// returned exactly the bar we expected" from "the provider returned some
/// other, still-valid, completed bar" (typically because the provider is
/// lagging behind the exchange). The manual poll-once route never supplies
/// this — its behavior is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpectedLatestBarConstraint {
    pub exact_end_ts: i64,
}

/// Outcome of one attempt to poll a provider's latest closed bar for one
/// already-admitted target and ingest it into `md_bars`.
#[derive(Debug, Clone)]
pub enum LatestBarPollOutcome {
    /// A genuinely new `md_bars` row was inserted.
    InsertedNewBar {
        end_ts: i64,
        rows_inserted: u64,
        rows_updated: u64,
        rows_skipped: u64,
    },
    /// The returned bar matched an existing `md_bars` row exactly (upsert
    /// touched an existing row, not a new one).
    AlreadyStored {
        end_ts: i64,
        rows_inserted: u64,
        rows_updated: u64,
        rows_skipped: u64,
    },
    /// REPAIR 4: an [`ExpectedLatestBarConstraint`] was supplied, the
    /// returned bar was ingested successfully, but its `end_ts` is older
    /// than the exact expected timestamp — the provider is lagging behind
    /// the exchange. The bar itself was still stored in `md_bars` (it is
    /// genuine, valid historical data), but it must never be treated as the
    /// caller's expected/observed bar.
    ProviderLaggingExpectedBar {
        returned_bar_ts: i64,
        expected_end_ts: i64,
    },
    /// REPAIR 4: an [`ExpectedLatestBarConstraint`] was supplied, the
    /// returned bar was ingested successfully, but its `end_ts` is newer
    /// than the exact expected timestamp — unexpected relative to the
    /// caller's own expected-bar computation. The bar was still stored.
    UnexpectedOrFutureBar {
        returned_bar_ts: i64,
        expected_end_ts: i64,
    },
    /// The provider had no bar to return for this reference time.
    NoCompletedBarAvailable,
    /// The provider capability/timeframe check failed before any call was
    /// made.
    Unsupported { detail: String },
    /// The provider call returned a non-transient error.
    ProviderRejected { detail: String },
    /// The provider call returned a transient error (rate-limited or
    /// temporarily unavailable) — safe to retry later.
    ProviderTemporarilyUnavailable { detail: String },
    /// The provider returned a bar, but it failed provenance/completeness/
    /// future-skew validation against the requested target.
    ProvenanceRejected {
        detail: String,
        returned_bar_ts: Option<i64>,
    },
    /// The DB ingest call itself failed.
    DatabaseFailure {
        detail: String,
        returned_bar_ts: i64,
    },
}

/// Input for [`poll_and_ingest_latest_closed_bar`]. `now_utc` is caller
/// supplied — this function never reads the wall clock.
pub struct LatestBarPollSeamInput<'a> {
    pub provider: &'a dyn mqk_md::MarketDataProvider,
    pub target: &'a ResolvedLatestBarPollTarget,
    pub now_utc: DateTime<Utc>,
    pub ingest_mode: &'a str,
    /// REPAIR 4: when `Some`, the returned bar's `end_ts` is checked against
    /// the exact expected timestamp after ingest (see
    /// [`LatestBarPollOutcome::ProviderLaggingExpectedBar`] /
    /// [`LatestBarPollOutcome::UnexpectedOrFutureBar`]). The manual
    /// poll-once route always supplies `None`, preserving its current
    /// behavior exactly.
    pub expected_bar_constraint: Option<ExpectedLatestBarConstraint>,
}

/// Poll one provider for its latest closed bar for `input.target`, validate
/// it, and ingest it into canonical `md_bars`. Reused, unchanged, by both
/// the manual poll-once route (per admitted symbol) and the Phase C
/// autonomous completed-bar driver (for its single effective binding).
pub async fn poll_and_ingest_latest_closed_bar(
    pool: &PgPool,
    input: LatestBarPollSeamInput<'_>,
) -> LatestBarPollOutcome {
    let capabilities = input.provider.capabilities();
    if !capabilities.latest_closed_bar {
        return LatestBarPollOutcome::Unsupported {
            detail: format!(
                "provider '{}' does not support capability latest_closed_bar",
                input.target.provider_id
            ),
        };
    }
    if !capabilities.supported_timeframes.is_empty()
        && !capabilities
            .supported_timeframes
            .contains(&input.target.timeframe)
    {
        return LatestBarPollOutcome::Unsupported {
            detail: format!(
                "provider '{}' does not support timeframe '{}'",
                input.target.provider_id,
                input.target.timeframe.as_str()
            ),
        };
    }

    let latest_expected_closed_bar_ts =
        mqk_md::latest_closed_bar_end_ts(input.target.timeframe, input.now_utc.timestamp());

    let request = mqk_md::LatestClosedBarRequest {
        symbol: input.target.provider_symbol.clone(),
        timeframe: input.target.timeframe,
        reference_ts: input.now_utc.timestamp(),
    };

    let latest_bar = match input.provider.fetch_latest_closed_bar(request).await {
        Ok(Some(bar)) => bar,
        Ok(None) => return LatestBarPollOutcome::NoCompletedBarAvailable,
        Err(error) => {
            return if matches!(
                error,
                mqk_md::MarketDataProviderError::RateLimited { .. }
                    | mqk_md::MarketDataProviderError::ProviderUnavailable { .. }
            ) {
                LatestBarPollOutcome::ProviderTemporarilyUnavailable {
                    detail: error.to_string(),
                }
            } else {
                LatestBarPollOutcome::ProviderRejected {
                    detail: error.to_string(),
                }
            };
        }
    };

    if !latest_bar
        .symbol
        .trim()
        .eq_ignore_ascii_case(input.target.provider_symbol.trim())
        || latest_bar.timeframe != input.target.timeframe.as_str()
        || !latest_bar.is_complete
        || latest_bar.end_ts > latest_expected_closed_bar_ts
    {
        return LatestBarPollOutcome::ProvenanceRejected {
            detail: "returned bar failed provenance/completeness/future-skew validation"
                .to_string(),
            returned_bar_ts: Some(latest_bar.end_ts),
        };
    }

    let ingest_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!(
            "mqk-md-latest-poll.v1|{}|{}|{}|{}",
            input.target.provider_id,
            input.target.timeframe.as_str(),
            input.target.local_symbol,
            latest_bar.end_ts
        )
        .as_bytes(),
    );

    let db_bar = mqk_db::md::ProviderBar {
        symbol: input.target.local_symbol.clone(),
        timeframe: latest_bar.timeframe,
        end_ts: latest_bar.end_ts,
        open: latest_bar.open,
        high: latest_bar.high,
        low: latest_bar.low,
        close: latest_bar.close,
        volume: latest_bar.volume,
        is_complete: latest_bar.is_complete,
    };
    let returned_bar_ts = db_bar.end_ts;

    match mqk_db::md::ingest_provider_bars_to_md_bars_with_provider_metadata(
        pool,
        mqk_db::md::IngestProviderBarsArgs {
            source: input.target.provider_id.clone(),
            timeframe: input.target.timeframe.as_str().to_string(),
            ingest_id,
            bars: vec![db_bar],
        },
        mqk_db::md::MdBarProviderMetadata {
            provider_id: input.target.provider_id.clone(),
            provider_source: Some(input.target.provider_id.clone()),
            provider_symbol: Some(input.target.provider_symbol.clone()),
            ingest_mode: Some(input.ingest_mode.to_string()),
            provider_bar_id: None,
            provider_updated_at_utc: None,
        },
    )
    .await
    {
        Ok(result) => {
            let rows_inserted = result.report.coverage.rows_inserted;
            let rows_updated = result.report.coverage.rows_updated;
            let rows_skipped = result.report.coverage.rows_rejected;

            // REPAIR 4: an exact autonomous constraint, when supplied,
            // reclassifies an otherwise-successful ingest as lagging/
            // unexpected relative to the caller's expected bar. The row was
            // already durably ingested above either way — this only affects
            // what the caller does with the outcome (never observed/
            // dispatched unless end_ts matches exactly).
            if let Some(constraint) = input.expected_bar_constraint {
                if returned_bar_ts < constraint.exact_end_ts {
                    return LatestBarPollOutcome::ProviderLaggingExpectedBar {
                        returned_bar_ts,
                        expected_end_ts: constraint.exact_end_ts,
                    };
                }
                if returned_bar_ts > constraint.exact_end_ts {
                    return LatestBarPollOutcome::UnexpectedOrFutureBar {
                        returned_bar_ts,
                        expected_end_ts: constraint.exact_end_ts,
                    };
                }
            }

            if rows_inserted > 0 {
                LatestBarPollOutcome::InsertedNewBar {
                    end_ts: returned_bar_ts,
                    rows_inserted,
                    rows_updated,
                    rows_skipped,
                }
            } else {
                LatestBarPollOutcome::AlreadyStored {
                    end_ts: returned_bar_ts,
                    rows_inserted,
                    rows_updated,
                    rows_skipped,
                }
            }
        }
        Err(error) => LatestBarPollOutcome::DatabaseFailure {
            detail: format!("db ingest failed: {error}"),
            returned_bar_ts,
        },
    }
}
