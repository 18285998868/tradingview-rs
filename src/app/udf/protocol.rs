//! UDF 响应结构。
//!
//! 字段名与 UDF/Charting Library 的 JSON 契约一一对应，避免在 handler 里
//! 用 `json!` 拼装时漏字段或写错键名。

use serde::Serialize;

use super::super::data_loader::Kline;
use super::resolution::Resolution;
use super::symbol::ParsedSymbol;

/// `/udf/time` 响应。
#[derive(Debug, Serialize)]
pub(crate) struct TimeResponse {
    /// 当前 UTC 秒时间戳。
    pub(crate) time: i64,
}

/// `/udf/config` 里的品种类型。
#[derive(Debug, Serialize)]
pub(crate) struct SymbolsType {
    pub(crate) name: &'static str,
    pub(crate) value: &'static str,
}

/// `/udf/config` 里的交易所。
#[derive(Debug, Serialize)]
pub(crate) struct Exchange {
    pub(crate) value: &'static str,
    pub(crate) name: &'static str,
    pub(crate) desc: &'static str,
}

/// `/udf/config` 响应。
#[derive(Debug, Serialize)]
pub(crate) struct ConfigResponse {
    pub(crate) supported_resolutions: Vec<&'static str>,
    pub(crate) supports_search: bool,
    pub(crate) supports_time: bool,
    pub(crate) supports_marks: bool,
    pub(crate) supports_timescale_marks: bool,
    pub(crate) supports_group_request: bool,
    pub(crate) supports_news: bool,
    pub(crate) symbols_types: Vec<SymbolsType>,
    pub(crate) exchanges: Vec<Exchange>,
}

impl ConfigResponse {
    /// 当前数据源能力。
    pub(crate) fn new() -> Self {
        Self {
            supported_resolutions: vec![
                "1", "3", "5", "15", "30", "60", "120", "240", "360", "480", "720", "1D", "3D",
                "1W", "1M",
            ],
            supports_search: true,
            supports_time: true,
            supports_marks: false,
            supports_timescale_marks: false,
            supports_group_request: false,
            supports_news: false,
            symbols_types: vec![
                SymbolsType {
                    name: "Crypto",
                    value: "crypto",
                },
                SymbolsType {
                    name: "Forex",
                    value: "forex",
                },
            ],
            exchanges: vec![
                Exchange {
                    value: "BINANCE",
                    name: "Binance Futures",
                    desc: "Binance UM futures",
                },
                Exchange {
                    value: "DUKASCOPY",
                    name: "Dukascopy",
                    desc: "Dukascopy FX / metals",
                },
            ],
        }
    }
}

/// `/udf/symbols` 响应。
#[derive(Debug, Serialize)]
pub(crate) struct SymbolInfo {
    pub(crate) name: String,
    pub(crate) full_name: String,
    pub(crate) ticker: String,
    pub(crate) description: String,
    #[serde(rename = "type")]
    pub(crate) symbol_type: &'static str,
    pub(crate) exchange: &'static str,
    pub(crate) listed_exchange: &'static str,
    pub(crate) session: &'static str,
    pub(crate) timezone: &'static str,
    pub(crate) format: &'static str,
    pub(crate) minmov: u64,
    pub(crate) pricescale: u64,
    pub(crate) has_intraday: bool,
    pub(crate) has_daily: bool,
    pub(crate) has_weekly_and_monthly: bool,
    pub(crate) has_empty_bars: bool,
    pub(crate) supported_resolutions: Vec<&'static str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) weekly_multipliers: Vec<&'static str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) monthly_multipliers: Vec<&'static str>,
}

impl SymbolInfo {
    /// 由 symbol、实际存在的 resolution 与推导出的 `pricescale` 构造。
    pub(crate) fn from_symbol(
        symbol: &ParsedSymbol,
        resolutions: &[Resolution],
        pricescale: u64,
    ) -> Self {
        let has_weekly = resolutions.iter().any(|r| r.is_week());
        let has_monthly = resolutions.iter().any(|r| r.is_month());
        let has_daily = resolutions.iter().any(|r| r.is_day());
        Self {
            name: symbol.pair.clone(),
            full_name: symbol.full_name(),
            ticker: symbol.pair.clone(),
            description: symbol.description(),
            symbol_type: symbol.symbol_type(),
            exchange: symbol.exchange(),
            listed_exchange: symbol.exchange(),
            session: symbol.session(),
            timezone: "Etc/UTC",
            format: "price",
            minmov: 1,
            pricescale,
            has_intraday: resolutions.iter().any(|r| r.is_intraday()),
            has_daily,
            has_weekly_and_monthly: has_weekly || has_monthly,
            has_empty_bars: false,
            supported_resolutions: resolutions.iter().map(|r| r.as_udf()).collect(),
            weekly_multipliers: if has_weekly { vec!["1"] } else { Vec::new() },
            monthly_multipliers: if has_monthly { vec!["1"] } else { Vec::new() },
        }
    }
}

/// `/udf/search` 的单项结果。
#[derive(Debug, Serialize)]
pub(crate) struct SearchItem {
    pub(crate) symbol: String,
    pub(crate) full_name: String,
    pub(crate) description: String,
    pub(crate) exchange: &'static str,
    pub(crate) ticker: String,
    #[serde(rename = "type")]
    pub(crate) symbol_type: &'static str,
}

impl SearchItem {
    /// 由 symbol 构造一条搜索结果。
    pub(crate) fn from_symbol(symbol: &ParsedSymbol) -> Self {
        Self {
            symbol: symbol.full_name(),
            full_name: symbol.full_name(),
            description: symbol.description(),
            exchange: symbol.exchange(),
            ticker: symbol.pair.clone(),
            symbol_type: symbol.symbol_type(),
        }
    }
}

/// UDF 错误体（`/udf/history` 与 `/udf/symbols` 共用）。
#[derive(Debug, Serialize)]
pub(crate) struct UdfError {
    pub(crate) s: &'static str,
    pub(crate) errmsg: String,
}

impl UdfError {
    /// 构造 `{ "s": "error", "errmsg": ... }`。
    pub(crate) fn new(errmsg: impl Into<String>) -> Self {
        Self {
            s: "error",
            errmsg: errmsg.into(),
        }
    }
}

/// `/udf/history` 响应。
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum HistoryResponse {
    /// `{ "s": "ok", "t": [...], ... }`
    Ok {
        s: &'static str,
        t: Vec<i64>,
        o: Vec<f64>,
        h: Vec<f64>,
        l: Vec<f64>,
        c: Vec<f64>,
        v: Vec<f64>,
    },
    /// `{ "s": "no_data", "nextTime": ... }`
    NoData {
        s: &'static str,
        #[serde(rename = "nextTime", skip_serializing_if = "Option::is_none")]
        next_time: Option<i64>,
    },
    /// `{ "s": "error", "errmsg": ... }`
    Error(UdfError),
}

impl HistoryResponse {
    /// 由毫秒时间戳的 K 线构造 `ok` 响应（UDF 用秒）。
    pub(crate) fn ok(klines: Vec<Kline>) -> Self {
        let mut t = Vec::with_capacity(klines.len());
        let mut o = Vec::with_capacity(klines.len());
        let mut h = Vec::with_capacity(klines.len());
        let mut l = Vec::with_capacity(klines.len());
        let mut c = Vec::with_capacity(klines.len());
        let mut v = Vec::with_capacity(klines.len());
        for k in klines {
            t.push(k.time.div_euclid(1_000));
            o.push(k.open);
            h.push(k.high);
            l.push(k.low);
            c.push(k.close);
            v.push(k.volume);
        }
        Self::Ok {
            s: "ok",
            t,
            o,
            h,
            l,
            c,
            v,
        }
    }

    /// `no_data` 响应（不带 `nextTime`）。
    pub(crate) fn no_data() -> Self {
        Self::NoData {
            s: "no_data",
            next_time: None,
        }
    }

    /// `no_data` 响应，并告知过去最近一根 bar 的时间（秒）。
    pub(crate) fn no_data_with_next_time(next_time: i64) -> Self {
        Self::NoData {
            s: "no_data",
            next_time: Some(next_time),
        }
    }

    /// `error` 响应。
    pub(crate) fn error(errmsg: impl Into<String>) -> Self {
        Self::Error(UdfError::new(errmsg))
    }
}
