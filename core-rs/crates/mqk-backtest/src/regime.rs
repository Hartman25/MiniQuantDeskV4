//! Pure research-only market regime detection over historical backtest bars.
//!
//! This module is intentionally detached from execution, routing, providers, DB,
//! and strategy behavior. It classifies supplied bars only.

use crate::types::BacktestBar;

#[derive(Clone, Debug, PartialEq)]
pub struct MarketRegimeInput {
    pub symbol: Option<String>,
    pub timeframe: Option<String>,
    pub bars: Vec<BacktestBar>,
}

impl MarketRegimeInput {
    pub fn from_bars(
        bars: Vec<BacktestBar>,
        symbol: Option<impl Into<String>>,
        timeframe: Option<impl Into<String>>,
    ) -> Self {
        Self {
            symbol: symbol.map(Into::into),
            timeframe: timeframe.map(Into::into),
            bars,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MarketRegimeFeatures {
    pub input_bar_count: usize,
    pub valid_bar_count: usize,
    pub return_pct: Option<f64>,
    pub realized_volatility_pct: Option<f64>,
    pub average_range_pct: Option<f64>,
    pub directional_consistency: Option<f64>,
    pub max_drawdown_pct: Option<f64>,
    pub volume_trend_pct: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MarketRegimeClassification {
    pub symbol: Option<String>,
    pub timeframe: Option<String>,
    pub bar_count: usize,
    pub kind: MarketRegimeKind,
    pub confidence: MarketRegimeConfidence,
    pub features: MarketRegimeFeatures,
    pub reason_codes: Vec<MarketRegimeReasonCode>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarketRegimeKind {
    BullTrend,
    BearTrend,
    Sideways,
    HighVolatility,
    LowVolatility,
    InsufficientData,
    Unknown,
}

impl MarketRegimeKind {
    pub fn code(self) -> &'static str {
        match self {
            MarketRegimeKind::BullTrend => "bull_trend",
            MarketRegimeKind::BearTrend => "bear_trend",
            MarketRegimeKind::Sideways => "sideways",
            MarketRegimeKind::HighVolatility => "high_volatility",
            MarketRegimeKind::LowVolatility => "low_volatility",
            MarketRegimeKind::InsufficientData => "insufficient_data",
            MarketRegimeKind::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MarketRegimeConfidence {
    pub score: f64,
}

impl MarketRegimeConfidence {
    pub fn new(score: f64) -> Self {
        Self {
            score: finite_or_zero(score).clamp(0.0, 1.0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarketRegimeReasonCode {
    ResearchOnly,
    InsufficientData,
    InvalidBarValue,
    IncompleteBarSkipped,
    PositiveReturn,
    NegativeReturn,
    DirectionalConsistency,
    LowReturn,
    ChoppyDirection,
    ModerateVolatility,
    HighVolatility,
    LowVolatility,
    MaxDrawdownElevated,
    NoRecognizedPattern,
}

impl MarketRegimeReasonCode {
    pub fn code(self) -> &'static str {
        match self {
            MarketRegimeReasonCode::ResearchOnly => "research_only",
            MarketRegimeReasonCode::InsufficientData => "insufficient_data",
            MarketRegimeReasonCode::InvalidBarValue => "invalid_bar_value",
            MarketRegimeReasonCode::IncompleteBarSkipped => "incomplete_bar_skipped",
            MarketRegimeReasonCode::PositiveReturn => "positive_return",
            MarketRegimeReasonCode::NegativeReturn => "negative_return",
            MarketRegimeReasonCode::DirectionalConsistency => "directional_consistency",
            MarketRegimeReasonCode::LowReturn => "low_return",
            MarketRegimeReasonCode::ChoppyDirection => "choppy_direction",
            MarketRegimeReasonCode::ModerateVolatility => "moderate_volatility",
            MarketRegimeReasonCode::HighVolatility => "high_volatility",
            MarketRegimeReasonCode::LowVolatility => "low_volatility",
            MarketRegimeReasonCode::MaxDrawdownElevated => "max_drawdown_elevated",
            MarketRegimeReasonCode::NoRecognizedPattern => "no_recognized_pattern",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MarketRegimePolicy {
    pub min_bars: usize,
    pub trend_return_threshold_pct: f64,
    pub sideways_return_threshold_pct: f64,
    pub directional_consistency_threshold: f64,
    pub high_volatility_threshold_pct: f64,
    pub high_range_threshold_pct: f64,
    pub low_volatility_threshold_pct: f64,
    pub low_range_threshold_pct: f64,
    pub elevated_drawdown_threshold_pct: f64,
}

impl MarketRegimePolicy {
    pub fn conservative_defaults() -> Self {
        Self {
            min_bars: 8,
            trend_return_threshold_pct: 5.0,
            sideways_return_threshold_pct: 2.0,
            directional_consistency_threshold: 0.70,
            high_volatility_threshold_pct: 6.0,
            high_range_threshold_pct: 8.0,
            low_volatility_threshold_pct: 0.20,
            low_range_threshold_pct: 0.50,
            elevated_drawdown_threshold_pct: 8.0,
        }
    }
}

pub fn detect_market_regime(
    input: &MarketRegimeInput,
    policy: &MarketRegimePolicy,
) -> MarketRegimeClassification {
    let mut reason_codes = vec![MarketRegimeReasonCode::ResearchOnly];
    let valid_bars = collect_valid_bars(&input.bars, &mut reason_codes);
    let features = calculate_features(input.bars.len(), &valid_bars);

    if valid_bars.len() < policy.min_bars {
        push_reason(&mut reason_codes, MarketRegimeReasonCode::InsufficientData);
        return classification(
            input,
            features,
            MarketRegimeKind::InsufficientData,
            0.0,
            reason_codes,
        );
    }

    let return_pct = features.return_pct.unwrap_or(0.0);
    let realized_volatility_pct = features.realized_volatility_pct.unwrap_or(0.0);
    let average_range_pct = features.average_range_pct.unwrap_or(0.0);
    let directional_consistency = features.directional_consistency.unwrap_or(0.0);
    let max_drawdown_pct = features.max_drawdown_pct.unwrap_or(0.0);

    if realized_volatility_pct >= policy.high_volatility_threshold_pct
        || average_range_pct >= policy.high_range_threshold_pct
    {
        push_reason(&mut reason_codes, MarketRegimeReasonCode::HighVolatility);
        if max_drawdown_pct >= policy.elevated_drawdown_threshold_pct {
            push_reason(
                &mut reason_codes,
                MarketRegimeReasonCode::MaxDrawdownElevated,
            );
        }
        return classification(
            input,
            features,
            MarketRegimeKind::HighVolatility,
            0.75,
            reason_codes,
        );
    }

    if return_pct >= policy.trend_return_threshold_pct
        && directional_consistency >= policy.directional_consistency_threshold
    {
        push_reason(&mut reason_codes, MarketRegimeReasonCode::PositiveReturn);
        push_reason(
            &mut reason_codes,
            MarketRegimeReasonCode::DirectionalConsistency,
        );
        return classification(
            input,
            features,
            MarketRegimeKind::BullTrend,
            0.85,
            reason_codes,
        );
    }

    if return_pct <= -policy.trend_return_threshold_pct
        && directional_consistency >= policy.directional_consistency_threshold
    {
        push_reason(&mut reason_codes, MarketRegimeReasonCode::NegativeReturn);
        push_reason(
            &mut reason_codes,
            MarketRegimeReasonCode::DirectionalConsistency,
        );
        if max_drawdown_pct >= policy.elevated_drawdown_threshold_pct {
            push_reason(
                &mut reason_codes,
                MarketRegimeReasonCode::MaxDrawdownElevated,
            );
        }
        return classification(
            input,
            features,
            MarketRegimeKind::BearTrend,
            0.85,
            reason_codes,
        );
    }

    if return_pct.abs() <= policy.sideways_return_threshold_pct
        && realized_volatility_pct <= policy.high_volatility_threshold_pct
        && average_range_pct <= policy.high_range_threshold_pct
    {
        push_reason(&mut reason_codes, MarketRegimeReasonCode::LowReturn);
        push_reason(&mut reason_codes, MarketRegimeReasonCode::ChoppyDirection);
        if realized_volatility_pct <= policy.low_volatility_threshold_pct
            && average_range_pct <= policy.low_range_threshold_pct
        {
            push_reason(&mut reason_codes, MarketRegimeReasonCode::LowVolatility);
            return classification(
                input,
                features,
                MarketRegimeKind::LowVolatility,
                0.70,
                reason_codes,
            );
        }
        push_reason(
            &mut reason_codes,
            MarketRegimeReasonCode::ModerateVolatility,
        );
        return classification(
            input,
            features,
            MarketRegimeKind::Sideways,
            0.70,
            reason_codes,
        );
    }

    push_reason(
        &mut reason_codes,
        MarketRegimeReasonCode::NoRecognizedPattern,
    );
    classification(
        input,
        features,
        MarketRegimeKind::Unknown,
        0.25,
        reason_codes,
    )
}

fn collect_valid_bars<'a>(
    bars: &'a [BacktestBar],
    reason_codes: &mut Vec<MarketRegimeReasonCode>,
) -> Vec<&'a BacktestBar> {
    let mut valid = Vec::with_capacity(bars.len());
    for bar in bars {
        if !bar.is_complete {
            push_reason(reason_codes, MarketRegimeReasonCode::IncompleteBarSkipped);
            continue;
        }
        if !bar_is_valid(bar) {
            push_reason(reason_codes, MarketRegimeReasonCode::InvalidBarValue);
            continue;
        }
        valid.push(bar);
    }
    valid
}

fn bar_is_valid(bar: &BacktestBar) -> bool {
    bar.open_micros > 0
        && bar.high_micros > 0
        && bar.low_micros > 0
        && bar.close_micros > 0
        && bar.high_micros >= bar.low_micros
        && bar.high_micros >= bar.open_micros
        && bar.high_micros >= bar.close_micros
        && bar.low_micros <= bar.open_micros
        && bar.low_micros <= bar.close_micros
}

fn calculate_features(input_bar_count: usize, bars: &[&BacktestBar]) -> MarketRegimeFeatures {
    let valid_bar_count = bars.len();
    if valid_bar_count < 2 {
        return MarketRegimeFeatures {
            input_bar_count,
            valid_bar_count,
            return_pct: None,
            realized_volatility_pct: None,
            average_range_pct: None,
            directional_consistency: None,
            max_drawdown_pct: None,
            volume_trend_pct: None,
        };
    }

    let first_close = micros_to_f64(bars[0].close_micros);
    let last_close = micros_to_f64(bars[valid_bar_count - 1].close_micros);
    let return_pct = percent_change(first_close, last_close);

    let mut returns = Vec::with_capacity(valid_bar_count.saturating_sub(1));
    let mut ranges_pct = Vec::with_capacity(valid_bar_count);
    let mut up_moves = 0usize;
    let mut down_moves = 0usize;

    for bar in bars {
        let high = micros_to_f64(bar.high_micros);
        let low = micros_to_f64(bar.low_micros);
        let close = micros_to_f64(bar.close_micros);
        if close > 0.0 {
            ranges_pct.push(((high - low) / close) * 100.0);
        }
    }

    for pair in bars.windows(2) {
        let prev = micros_to_f64(pair[0].close_micros);
        let next = micros_to_f64(pair[1].close_micros);
        if prev > 0.0 {
            let ret = (next - prev) / prev;
            if ret > 0.0 {
                up_moves += 1;
            } else if ret < 0.0 {
                down_moves += 1;
            }
            returns.push(ret);
        }
    }

    let realized_volatility_pct = stddev(&returns).map(|v| v * 100.0);
    let average_range_pct = average(&ranges_pct);
    let directional_consistency = return_pct.map(|r| {
        let total = returns.len().max(1) as f64;
        if r > 0.0 {
            up_moves as f64 / total
        } else if r < 0.0 {
            down_moves as f64 / total
        } else {
            up_moves.max(down_moves) as f64 / total
        }
    });
    let max_drawdown_pct = max_drawdown_pct(bars);
    let volume_trend_pct = volume_trend_pct(bars);

    MarketRegimeFeatures {
        input_bar_count,
        valid_bar_count,
        return_pct: return_pct.filter(|v| v.is_finite()),
        realized_volatility_pct: realized_volatility_pct.filter(|v| v.is_finite()),
        average_range_pct: average_range_pct.filter(|v| v.is_finite()),
        directional_consistency: directional_consistency.filter(|v| v.is_finite()),
        max_drawdown_pct: max_drawdown_pct.filter(|v| v.is_finite()),
        volume_trend_pct: volume_trend_pct.filter(|v| v.is_finite()),
    }
}

fn classification(
    input: &MarketRegimeInput,
    features: MarketRegimeFeatures,
    kind: MarketRegimeKind,
    confidence: f64,
    reason_codes: Vec<MarketRegimeReasonCode>,
) -> MarketRegimeClassification {
    MarketRegimeClassification {
        symbol: input.symbol.clone(),
        timeframe: input.timeframe.clone(),
        bar_count: input.bars.len(),
        kind,
        confidence: MarketRegimeConfidence::new(confidence),
        features,
        reason_codes,
    }
}

fn micros_to_f64(value: i64) -> f64 {
    value as f64 / 1_000_000.0
}

fn percent_change(first: f64, last: f64) -> Option<f64> {
    if first > 0.0 && first.is_finite() && last.is_finite() {
        Some(((last - first) / first) * 100.0)
    } else {
        None
    }
}

fn average(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let sum: f64 = values.iter().copied().sum();
    Some(sum / values.len() as f64)
}

fn stddev(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mean = average(values)?;
    let variance = values
        .iter()
        .map(|value| {
            let delta = value - mean;
            delta * delta
        })
        .sum::<f64>()
        / values.len() as f64;
    Some(variance.sqrt())
}

fn max_drawdown_pct(bars: &[&BacktestBar]) -> Option<f64> {
    let mut peak = micros_to_f64(bars.first()?.close_micros);
    let mut max_drawdown = 0.0;
    for bar in bars {
        let close = micros_to_f64(bar.close_micros);
        if close > peak {
            peak = close;
        }
        if peak > 0.0 {
            let drawdown = ((peak - close) / peak) * 100.0;
            if drawdown > max_drawdown {
                max_drawdown = drawdown;
            }
        }
    }
    Some(max_drawdown)
}

fn volume_trend_pct(bars: &[&BacktestBar]) -> Option<f64> {
    let first = bars.first()?.volume;
    let last = bars.last()?.volume;
    if first > 0 && last >= 0 {
        percent_change(first as f64, last as f64)
    } else {
        None
    }
}

fn push_reason(
    reason_codes: &mut Vec<MarketRegimeReasonCode>,
    reason_code: MarketRegimeReasonCode,
) {
    if !reason_codes.contains(&reason_code) {
        reason_codes.push(reason_code);
    }
}

fn finite_or_zero(value: f64) -> f64 {
    if value.is_finite() {
        value
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(end_ts: i64, open: i64, high: i64, low: i64, close: i64) -> BacktestBar {
        BacktestBar::new("SPY", end_ts, open, high, low, close, 1_000)
    }

    fn classify(bars: Vec<BacktestBar>) -> MarketRegimeClassification {
        let input = MarketRegimeInput::from_bars(bars, Some("SPY"), Some("1m"));
        detect_market_regime(&input, &MarketRegimePolicy::conservative_defaults())
    }

    #[test]
    fn regime_strong_upward_trend_is_bull_trend() {
        let bars = (0..10)
            .map(|i| {
                let close = 100_000_000 + i * 1_000_000;
                bar(i, close - 250_000, close + 500_000, close - 500_000, close)
            })
            .collect();

        let result = classify(bars);

        assert_eq!(result.kind, MarketRegimeKind::BullTrend);
        assert!(result.confidence.score > 0.0);
        assert!(result
            .reason_codes
            .contains(&MarketRegimeReasonCode::PositiveReturn));
    }

    #[test]
    fn regime_strong_downward_trend_is_bear_trend() {
        let bars = (0..10)
            .map(|i| {
                let close = 110_000_000 - i * 1_000_000;
                bar(i, close + 250_000, close + 500_000, close - 500_000, close)
            })
            .collect();

        let result = classify(bars);

        assert_eq!(result.kind, MarketRegimeKind::BearTrend);
        assert!(result
            .reason_codes
            .contains(&MarketRegimeReasonCode::NegativeReturn));
    }

    #[test]
    fn regime_flat_choppy_data_is_sideways() {
        let closes = [
            100_000_000,
            101_000_000,
            99_000_000,
            100_500_000,
            99_500_000,
            100_250_000,
            99_750_000,
            100_000_000,
        ];
        let bars = closes
            .iter()
            .enumerate()
            .map(|(i, close)| {
                bar(
                    i as i64,
                    *close,
                    close + 1_000_000,
                    close - 1_000_000,
                    *close,
                )
            })
            .collect();

        let result = classify(bars);

        assert_eq!(result.kind, MarketRegimeKind::Sideways);
        assert!(result
            .reason_codes
            .contains(&MarketRegimeReasonCode::LowReturn));
    }

    #[test]
    fn regime_high_volatility_is_classified_or_reasoned() {
        let closes = [
            100_000_000,
            115_000_000,
            90_000_000,
            120_000_000,
            85_000_000,
            125_000_000,
            95_000_000,
            110_000_000,
        ];
        let bars = closes
            .iter()
            .enumerate()
            .map(|(i, close)| {
                bar(
                    i as i64,
                    *close,
                    close + 8_000_000,
                    close - 8_000_000,
                    *close,
                )
            })
            .collect();

        let result = classify(bars);

        assert_eq!(result.kind, MarketRegimeKind::HighVolatility);
        assert!(result
            .reason_codes
            .contains(&MarketRegimeReasonCode::HighVolatility));
    }

    #[test]
    fn regime_too_few_bars_is_insufficient_data() {
        let result = classify(vec![
            bar(1, 100_000_000, 101_000_000, 99_000_000, 100_000_000),
            bar(2, 101_000_000, 102_000_000, 100_000_000, 101_000_000),
        ]);

        assert_eq!(result.kind, MarketRegimeKind::InsufficientData);
        assert!(result
            .reason_codes
            .contains(&MarketRegimeReasonCode::InsufficientData));
    }

    #[test]
    fn regime_bad_values_do_not_panic_and_are_reported() {
        let bars = vec![
            bar(1, 100_000_000, 101_000_000, 99_000_000, 100_000_000),
            bar(2, 0, 101_000_000, 99_000_000, 100_000_000),
            bar(3, 101_000_000, 99_000_000, 100_000_000, 100_000_000),
        ];

        let result = classify(bars);

        assert_eq!(result.kind, MarketRegimeKind::InsufficientData);
        assert!(result
            .reason_codes
            .contains(&MarketRegimeReasonCode::InvalidBarValue));
    }

    #[test]
    fn regime_output_is_deterministic_for_same_input() {
        let bars: Vec<_> = (0..10)
            .map(|i| {
                let close = 100_000_000 + i * 1_000_000;
                bar(i, close - 250_000, close + 500_000, close - 500_000, close)
            })
            .collect();

        let first = classify(bars.clone());
        let second = classify(bars);

        assert_eq!(first, second);
    }
}
