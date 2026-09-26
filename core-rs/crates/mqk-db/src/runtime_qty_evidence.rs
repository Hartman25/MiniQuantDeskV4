//! Shared durable quantity-evidence codec for the Bundle 5 / Bundle 6
//! runtime evidence tables.
//!
//! New rows are written as `quantity_schema_version = 'qty_micros_v1'` with
//! the raw [`QtyMicros`] value in the `*_micros` columns and the legacy
//! whole-unit columns `NULL`. Historical rows carry a `NULL` schema version
//! and only the legacy whole-unit columns; they decode through the checked
//! [`QtyMicros::from_whole_units`] conversion. Any other combination (mixed
//! authority, unknown schema version) fails closed.
//!
//! The quantity scale authority is `mqk_schemas::QTY_MICROS_SCALE` (via
//! `QtyMicros`); this module defines no scale of its own.

use anyhow::{anyhow, Context, Result};
use mqk_schemas::QtyMicros;

/// Encoding tag written by every new evidence candidate row.
pub const RUNTIME_QTY_EVIDENCE_SCHEMA_MICROS_V1: &str = "qty_micros_v1";

/// Checked conversion of a historical whole-unit value.
pub(crate) fn decode_legacy_whole(field: &str, value: i64) -> Result<QtyMicros> {
    QtyMicros::from_whole_units(value).ok_or_else(|| {
        anyhow!("historical whole-unit quantity {value} in {field} overflows QtyMicros")
    })
}

fn read_pair(
    row: &sqlx::postgres::PgRow,
    legacy_field: &str,
    micros_field: &str,
) -> Result<(Option<i64>, Option<i64>)> {
    use sqlx::Row;
    let legacy: Option<i64> = row
        .try_get(legacy_field)
        .with_context(|| format!("read legacy quantity field {legacy_field}"))?;
    let micros: Option<i64> = row
        .try_get(micros_field)
        .with_context(|| format!("read QtyMicros field {micros_field}"))?;
    Ok((legacy, micros))
}

pub(crate) fn decode_required_quantity(
    row: &sqlx::postgres::PgRow,
    quantity_schema_version: Option<&str>,
    legacy_field: &str,
    micros_field: &str,
) -> Result<QtyMicros> {
    let (legacy, micros) = read_pair(row, legacy_field, micros_field)?;
    match quantity_schema_version {
        None => match (legacy, micros) {
            (Some(value), None) => decode_legacy_whole(legacy_field, value),
            _ => Err(anyhow!(
                "historical quantity row has mixed/missing authority for {legacy_field}/{micros_field}"
            )),
        },
        Some(RUNTIME_QTY_EVIDENCE_SCHEMA_MICROS_V1) => match (legacy, micros) {
            (None, Some(value)) => Ok(QtyMicros::new(value)),
            _ => Err(anyhow!(
                "qty_micros_v1 row has mixed/missing authority for {legacy_field}/{micros_field}"
            )),
        },
        Some(other) => Err(anyhow!("unsupported quantity_schema_version '{other}'")),
    }
}

pub(crate) fn decode_optional_quantity(
    row: &sqlx::postgres::PgRow,
    quantity_schema_version: Option<&str>,
    legacy_field: &str,
    micros_field: &str,
) -> Result<Option<QtyMicros>> {
    let (legacy, micros) = read_pair(row, legacy_field, micros_field)?;
    match quantity_schema_version {
        None => {
            if micros.is_some() {
                return Err(anyhow!(
                    "historical quantity row carries unexpected QtyMicros authority in {micros_field}"
                ));
            }
            legacy
                .map(|value| decode_legacy_whole(legacy_field, value))
                .transpose()
        }
        Some(RUNTIME_QTY_EVIDENCE_SCHEMA_MICROS_V1) => {
            if legacy.is_some() {
                return Err(anyhow!(
                    "qty_micros_v1 row carries unexpected legacy quantity authority in {legacy_field}"
                ));
            }
            Ok(micros.map(QtyMicros::new))
        }
        Some(other) => Err(anyhow!("unsupported quantity_schema_version '{other}'")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_whole_conversion_is_checked_and_scaled_by_the_canonical_scale() {
        assert_eq!(
            decode_legacy_whole("qty", 2).unwrap(),
            QtyMicros::new(2 * mqk_schemas::QTY_MICROS_SCALE)
        );
        assert_eq!(decode_legacy_whole("qty", -3).unwrap().raw(), -3_000_000);
        assert!(decode_legacy_whole("qty", i64::MAX).is_err());
        assert!(decode_legacy_whole("qty", i64::MIN).is_err());
    }
}
