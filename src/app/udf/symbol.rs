//! UDF symbol 解析与交易所元数据。

use super::super::data_loader::MarketType;
use crate::utils::valid_pair;

/// 解析后的 UDF symbol。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedSymbol {
    pub(crate) market: MarketType,
    pub(crate) pair: String,
}

impl ParsedSymbol {
    pub(crate) fn exchange(&self) -> &'static str {
        match self.market {
            MarketType::Dukascopy => "DUKASCOPY",
            MarketType::Futures | MarketType::Spot => "BINANCE",
        }
    }

    pub(crate) fn symbol_type(&self) -> &'static str {
        match self.market {
            MarketType::Dukascopy => "forex",
            MarketType::Futures | MarketType::Spot => "crypto",
        }
    }

    pub(crate) fn session(&self) -> &'static str {
        match self.market {
            // 24/5：周一到周五、整天，UTC 下用 0000-0000。
            MarketType::Dukascopy => "0000-0000",
            MarketType::Futures | MarketType::Spot => "24x7",
        }
    }

    pub(crate) fn description(&self) -> String {
        match self.market {
            MarketType::Dukascopy => format!("{} · Dukascopy", self.pair),
            MarketType::Futures | MarketType::Spot => format!("{} · Binance Futures", self.pair),
        }
    }

    pub(crate) fn full_name(&self) -> String {
        format!("{}:{}", self.exchange(), self.pair)
    }

    /// UDF `pricescale`（价格小数位）。
    ///
    /// 静态值只作下限；`/udf/symbols` 会由 `price::infer` 按样本价格抬高精度。
    pub(crate) fn pricescale(&self) -> u64 {
        match self.market {
            MarketType::Dukascopy if self.pair.ends_with("JPY") => 1_000,
            MarketType::Dukascopy
                if self.pair.starts_with("XAU") || self.pair.starts_with("XAG") =>
            {
                100
            }
            MarketType::Dukascopy => 100_000,
            MarketType::Futures | MarketType::Spot => 100,
        }
    }
}

/// symbol 解析失败。
#[derive(Debug, thiserror::Error)]
pub(crate) enum SymbolError {
    /// symbol 为空。
    #[error("symbol is empty")]
    Empty,
    /// 交易对不合法。
    #[error("invalid symbol: {0}")]
    Invalid(String),
    /// 未知交易所前缀。
    #[error("unknown exchange: {0}")]
    UnknownExchange(String),
}

/// 解析 UDF symbol；裸交易对默认按 futures 处理。
pub(crate) fn parse_symbol(raw: &str) -> Result<ParsedSymbol, SymbolError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(SymbolError::Empty);
    }
    let (prefix, pair_raw) = match raw.split_once(':') {
        Some((prefix, pair)) => (prefix.trim(), pair.trim()),
        None => ("", raw),
    };
    let pair = pair_raw.to_ascii_uppercase();
    if !valid_pair(&pair) {
        return Err(SymbolError::Invalid(raw.to_string()));
    }
    let market = match prefix.to_ascii_uppercase().as_str() {
        "" => MarketType::Futures,
        "BINANCE" | "BINANCEFUTURES" | "BINANCE_FUTURES" | "BINANCE-FUTURES" | "FUTURES" => {
            MarketType::Futures
        }
        "DUKASCOPY" | "FX" => MarketType::Dukascopy,
        other => return Err(SymbolError::UnknownExchange(other.to_string())),
    };
    Ok(ParsedSymbol { market, pair })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_prefixed_and_bare_symbols() {
        let p = parse_symbol("BINANCE:BTCUSDT").unwrap();
        assert_eq!(p.market, MarketType::Futures);
        assert_eq!(p.pair, "BTCUSDT");
        assert_eq!(p.full_name(), "BINANCE:BTCUSDT");

        let p = parse_symbol("dukascopy:eurusd").unwrap();
        assert_eq!(p.market, MarketType::Dukascopy);
        assert_eq!(p.pair, "EURUSD");
        assert_eq!(p.full_name(), "DUKASCOPY:EURUSD");
        // 24/5：UTC 下用 0000-0000，不是 24x5。
        assert_eq!(p.session(), "0000-0000");
        assert_eq!(parse_symbol("BINANCE:BTCUSDT").unwrap().session(), "24x7");

        assert!(parse_symbol("").is_err());
        assert!(parse_symbol("../etc").is_err());
        assert!(parse_symbol("NYSE:BTCUSDT").is_err());
    }

    #[test]
    fn pricescale_matches_fx_quote_precision() {
        assert_eq!(
            parse_symbol("DUKASCOPY:EURUSD").unwrap().pricescale(),
            100_000
        );
        assert_eq!(
            parse_symbol("DUKASCOPY:USDJPY").unwrap().pricescale(),
            1_000
        );
        assert_eq!(parse_symbol("DUKASCOPY:XAUUSD").unwrap().pricescale(), 100);
        assert_eq!(parse_symbol("BINANCE:BTCUSDT").unwrap().pricescale(), 100);
    }
}
