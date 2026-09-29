//! Canonical US equity-option contract identity: the ONE typed parse/format
//! of an OCC/provider option symbol.
//!
//! An OCC symbol is `ROOT(1-6 letters) YYMMDD (C|P) STRIKE(8 digits, price*1000)`,
//! e.g. `AAPL230721C00150000`. Alpaca sends it with no padding. Only the
//! standard 100-share contract is representable here: an adjusted/non-standard
//! root (digits, or any other shape) is refused rather than guessed, because
//! its deliverable and multiplier are not derivable from the symbol.
//!
//! Every consumer that needs strike/right/expiry/underlying (the options MLEG
//! wire, lifecycle apply, the lifecycle pending gate) goes through
//! [`OptionContractIdentity`]; nothing else parses an option symbol.

/// Shares per standard US equity-option contract.
pub const STANDARD_OPTION_MULTIPLIER: i64 = 100;

/// The repo's existing option right type.
pub use mqk_schemas::OptionRight;

fn occ_char(right: OptionRight) -> char {
    match right {
        OptionRight::Call => 'C',
        OptionRight::Put => 'P',
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionContractParseError {
    /// Not `ROOT YYMMDD C|P 8-digit-strike` with a 1-6 letter root.
    Shape(String),
    /// Calendar date is not a real date.
    InvalidExpiration(String),
    /// Strike is zero.
    ZeroStrike(String),
}

impl std::fmt::Display for OptionContractParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shape(s) => write!(f, "not a standard OCC option symbol: {s:?}"),
            Self::InvalidExpiration(s) => write!(f, "OCC symbol has an invalid expiration: {s:?}"),
            Self::ZeroStrike(s) => write!(f, "OCC symbol has a zero strike: {s:?}"),
        }
    }
}

impl std::error::Error for OptionContractParseError {}

/// Parsed, validated option contract identity. Fields are private so a value
/// exists only if it came from [`OptionContractIdentity::parse`] or
/// [`OptionContractIdentity::new`], both of which validate.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OptionContractIdentity {
    underlying: String,
    expiration_year: u16,
    expiration_month: u8,
    expiration_day: u8,
    right: OptionRight,
    /// Strike price * 1000 (OCC's 8-digit field).
    strike_thousandths: u32,
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            let leap = match (year % 4, year % 100, year % 400) {
                (_, _, 0) => true,
                (_, 0, _) => false,
                (0, _, _) => true,
                _ => false,
            };
            if leap {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

impl OptionContractIdentity {
    pub fn new(
        underlying: &str,
        expiration_year: u16,
        expiration_month: u8,
        expiration_day: u8,
        right: OptionRight,
        strike_thousandths: u32,
    ) -> Result<Self, OptionContractParseError> {
        let shape = || {
            format!(
                "{underlying}/{expiration_year}-{expiration_month}-{expiration_day}/{strike_thousandths}"
            )
        };
        if underlying.is_empty()
            || underlying.len() > 6
            || !underlying.chars().all(|c| c.is_ascii_uppercase())
        {
            return Err(OptionContractParseError::Shape(shape()));
        }
        if !(2000..=2099).contains(&expiration_year)
            || expiration_day == 0
            || expiration_day > days_in_month(expiration_year, expiration_month)
        {
            return Err(OptionContractParseError::InvalidExpiration(shape()));
        }
        if strike_thousandths == 0 || strike_thousandths > 99_999_999 {
            return Err(OptionContractParseError::ZeroStrike(shape()));
        }
        Ok(Self {
            underlying: underlying.to_string(),
            expiration_year,
            expiration_month,
            expiration_day,
            right,
            strike_thousandths,
        })
    }

    /// Strict parse: exact canonical form only (no case folding, no padding,
    /// no whitespace), so `parse(s).occ_symbol() == s` for every accepted `s`.
    pub fn parse(symbol: &str) -> Result<Self, OptionContractParseError> {
        let bad = || OptionContractParseError::Shape(symbol.to_string());
        if !symbol.is_ascii() || symbol.len() < 16 || symbol.len() > 21 {
            return Err(bad());
        }
        let root_len = symbol.len() - 15;
        let (root, rest) = symbol.split_at(root_len);
        let (date, rest) = rest.split_at(6);
        let (right, strike) = rest.split_at(1);
        if !date.chars().all(|c| c.is_ascii_digit()) || !strike.chars().all(|c| c.is_ascii_digit())
        {
            return Err(bad());
        }
        let right = match right {
            "C" => OptionRight::Call,
            "P" => OptionRight::Put,
            _ => return Err(bad()),
        };
        let yy: u16 = date[0..2].parse().map_err(|_| bad())?;
        let mm: u8 = date[2..4].parse().map_err(|_| bad())?;
        let dd: u8 = date[4..6].parse().map_err(|_| bad())?;
        let strike_thousandths: u32 = strike.parse().map_err(|_| bad())?;
        Self::new(root, 2000 + yy, mm, dd, right, strike_thousandths)
    }

    pub fn underlying(&self) -> &str {
        &self.underlying
    }

    pub fn right(&self) -> OptionRight {
        self.right
    }

    pub fn is_call(&self) -> bool {
        self.right == OptionRight::Call
    }

    pub fn expiration(&self) -> (u16, u8, u8) {
        (
            self.expiration_year,
            self.expiration_month,
            self.expiration_day,
        )
    }

    /// `YYYY-MM-DD`.
    pub fn expiration_iso(&self) -> String {
        format!(
            "{:04}-{:02}-{:02}",
            self.expiration_year, self.expiration_month, self.expiration_day
        )
    }

    pub fn strike_thousandths(&self) -> u32 {
        self.strike_thousandths
    }

    /// Strike in 1e-6 dollars (the repo's price-micros scale). Exact.
    pub fn strike_micros(&self) -> i64 {
        i64::from(self.strike_thousandths) * 1_000
    }

    pub fn multiplier(&self) -> i64 {
        STANDARD_OPTION_MULTIPLIER
    }

    /// Canonical OCC symbol (the exact provider wire symbol).
    pub fn occ_symbol(&self) -> String {
        format!(
            "{}{:02}{:02}{:02}{}{:08}",
            self.underlying,
            self.expiration_year % 100,
            self.expiration_month,
            self.expiration_day,
            occ_char(self.right),
            self.strike_thousandths
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_roundtrips_the_canonical_symbol() {
        for s in [
            "AAPL230721C00150000",
            "SPY260619P00500000",
            "F260116C00012500",
            "GOOGL240119C02800000",
            "AAPL240229P00150500",
        ] {
            let c = OptionContractIdentity::parse(s).unwrap();
            assert_eq!(c.occ_symbol(), s);
        }
    }

    #[test]
    fn fields_are_exact() {
        let c = OptionContractIdentity::parse("AAPL230721C00150000").unwrap();
        assert_eq!(c.underlying(), "AAPL");
        assert!(c.is_call());
        assert_eq!(c.expiration(), (2023, 7, 21));
        assert_eq!(c.expiration_iso(), "2023-07-21");
        assert_eq!(c.strike_micros(), 150_000_000);
        assert_eq!(c.multiplier(), 100);
        let p = OptionContractIdentity::parse("SPY260619P00512500").unwrap();
        assert_eq!(p.right(), OptionRight::Put);
        assert_eq!(p.strike_micros(), 512_500_000);
    }

    #[test]
    fn the_underlying_ticker_is_never_an_option_contract() {
        assert!(OptionContractIdentity::parse("AAPL").is_err());
        assert!(OptionContractIdentity::parse("BTC/USD").is_err());
    }

    #[test]
    fn non_canonical_or_non_standard_shapes_are_refused() {
        for s in [
            "",
            "aapl230721C00150000",    // lowercase root
            "AAPL230721c00150000",    // lowercase right
            "AAPL230721X00150000",    // bad right
            "AAPL230231C00150000",    // Feb 31
            "AAPL231301C00150000",    // month 13
            "AAPL230700C00150000",    // day 0
            "AAPL230721C00000000",    // zero strike
            "AAPL230721C0015000",     // short strike
            "AAPL230721C001500000",   // long strike
            "AAPL1230721C00150000",   // adjusted (digit) root
            "AAPLXYZ230721C00150000", // root > 6
            " AAPL230721C00150000",
            "AAPL 230721C00150000",
        ] {
            assert!(
                OptionContractIdentity::parse(s).is_err(),
                "{s:?} must refuse"
            );
        }
    }

    #[test]
    fn leap_day_is_validated_by_year() {
        assert!(OptionContractIdentity::parse("AAPL240229C00150000").is_ok());
        assert!(OptionContractIdentity::parse("AAPL230229C00150000").is_err());
    }
}
