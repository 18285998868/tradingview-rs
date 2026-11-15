//! `/udf/*` 端点实现。
//!
//! 读盘与目录扫描全部走 `spawn_blocking`；handler 只做参数解析与响应映射。

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;

use super::super::data_loader::{DataError, MarketType};
use super::super::routes::AppState;
use super::history::HistoryWindow;
use super::price;
use super::protocol::{
    ConfigResponse, HistoryResponse, SearchItem, SymbolInfo, TimeResponse, UdfError,
};
use super::resolution::{Resolution, timeframes_to_resolutions};
use super::symbol::{ParsedSymbol, parse_symbol};

/// `/udf/search` 默认返回条数。
const DEFAULT_SEARCH_LIMIT: usize = 30;
/// `/udf/search` 最大返回条数。
const MAX_SEARCH_LIMIT: usize = 100;

/// `/udf/history` 查询参数。
#[derive(Debug, Deserialize)]
pub(crate) struct HistoryQuery {
    /// `BINANCE:BTCUSDT` / `DUKASCOPY:EURUSD` / 裸交易对（默认 futures）。
    pub(crate) symbol: String,
    /// UDF 周期：`1`、`5`、`60`、`240`、`1D`…
    pub(crate) resolution: String,
    /// 起始时间（秒，含）。
    pub(crate) from: Option<i64>,
    /// 结束时间（秒，右开区间，不含）。
    pub(crate) to: Option<i64>,
    /// 可选：最多返回多少根。
    pub(crate) countback: Option<usize>,
}

/// `/udf/symbols` 查询参数。
#[derive(Debug, Deserialize)]
pub(crate) struct SymbolQuery {
    /// UDF symbol。
    pub(crate) symbol: String,
}

/// `/udf/search` 查询参数。
#[derive(Debug, Deserialize)]
pub(crate) struct SearchQuery {
    /// 交易对子串（大小写不敏感）。
    pub(crate) query: Option<String>,
    /// 品种类型过滤：`crypto` / `forex`。
    #[serde(rename = "type")]
    pub(crate) symbol_type: Option<String>,
    /// 交易所过滤：`BINANCE` / `DUKASCOPY`。
    pub(crate) exchange: Option<String>,
    /// 返回条数上限。
    pub(crate) limit: Option<usize>,
}

/// 当前服务器时间（UDF 用秒）。
pub(crate) async fn time_handler() -> Json<TimeResponse> {
    Json(TimeResponse {
        time: chrono::Utc::now().timestamp(),
    })
}

/// `/udf/config`。
pub(crate) async fn config_handler() -> Json<ConfigResponse> {
    Json(ConfigResponse::new())
}

/// 数据层错误 → UDF 响应。
fn handle_history_error(e: DataError) -> HistoryResponse {
    match &e {
        DataError::NotFound(_) => HistoryResponse::no_data(),
        DataError::InvalidParam(msg) => HistoryResponse::error(msg.clone()),
        _ => {
            tracing::error!(error = %e, "udf history failed");
            HistoryResponse::error("internal error")
        }
    }
}

/// 窗口为空时回查过去最近一根 bar 的时间（秒）。
async fn next_time_after(
    state: &AppState,
    market: MarketType,
    pair: &str,
    timeframe: &'static str,
    anchor: i64,
) -> Option<i64> {
    let loader = state.data_loader.clone();
    let pair = pair.to_owned();
    let joined = tokio::task::spawn_blocking(move || {
        loader.load_klines_filtered(market, &pair, timeframe, Some(anchor), None, Some(1))
    })
    .await;
    match joined {
        Ok(Ok(klines)) => klines.last().map(|k| k.time.div_euclid(1_000)),
        Ok(Err(e)) => {
            tracing::debug!(error = %e, "udf next_time lookup failed");
            None
        }
        Err(e) => {
            tracing::debug!(error = %e, "udf next_time task failed");
            None
        }
    }
}

/// `/udf/history`：返回 UDF 要求的 `t/o/h/l/c/v` 秒级数组。
pub(crate) async fn history_handler(
    State(state): State<AppState>,
    Query(query): Query<HistoryQuery>,
) -> impl IntoResponse {
    let parsed = match parse_symbol(&query.symbol) {
        Ok(p) => p,
        Err(e) => return (StatusCode::OK, Json(HistoryResponse::error(e.to_string()))),
    };
    let resolved = match Resolution::parse(parsed.market, &query.resolution) {
        Ok(r) => r,
        Err(e) => return (StatusCode::OK, Json(HistoryResponse::error(e.to_string()))),
    };
    let window = match HistoryWindow::new(query.from, query.to, query.countback) {
        Ok(w) => w,
        Err(e) => return (StatusCode::OK, Json(HistoryResponse::error(e.to_string()))),
    };
    let (before, after, limit) = window.loader_args();

    let joined = tokio::task::spawn_blocking({
        let loader = state.data_loader.clone();
        let pair = parsed.pair.clone();
        let timeframe = resolved.timeframe;
        move || loader.load_klines_filtered(parsed.market, &pair, timeframe, before, after, limit)
    })
    .await;

    match joined {
        Ok(Ok(klines)) => {
            if klines.is_empty() {
                let next_time = match window.next_time_anchor() {
                    Some(anchor) => {
                        next_time_after(
                            &state,
                            parsed.market,
                            &parsed.pair,
                            resolved.timeframe,
                            anchor,
                        )
                        .await
                    }
                    None => None,
                };
                let body = match next_time {
                    Some(t) => HistoryResponse::no_data_with_next_time(t),
                    None => HistoryResponse::no_data(),
                };
                return (StatusCode::OK, Json(body));
            }
            (StatusCode::OK, Json(HistoryResponse::ok(klines)))
        }
        Ok(Err(e)) => (StatusCode::OK, Json(handle_history_error(e))),
        Err(e) => {
            tracing::error!(error = %e, "udf history task failed");
            (
                StatusCode::OK,
                Json(HistoryResponse::error("internal error")),
            )
        }
    }
}

/// `/udf/symbols`：返回 Charting Library 需要的 symbol info。
pub(crate) async fn symbols_handler(
    State(state): State<AppState>,
    Query(query): Query<SymbolQuery>,
) -> Response {
    let parsed = match parse_symbol(&query.symbol) {
        Ok(p) => p,
        Err(e) => {
            return (StatusCode::NOT_FOUND, Json(UdfError::new(e.to_string()))).into_response();
        }
    };
    let market = parsed.market;
    let pair = parsed.pair.clone();
    let fallback = parsed.pricescale();
    let joined = tokio::task::spawn_blocking({
        let loader = state.data_loader.clone();
        move || {
            let timeframes = loader.list_timeframes(market, &pair)?;
            let pricescale = price::infer(&loader, market, &pair, &timeframes, fallback);
            Ok::<_, DataError>((timeframes, pricescale))
        }
    })
    .await;

    let (timeframes, pricescale) = match joined {
        Ok(Ok(tfs)) => tfs,
        Ok(Err(e)) => {
            tracing::error!(error = %e, symbol = %query.symbol, "udf symbols failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(UdfError::new("internal error")),
            )
                .into_response();
        }
        Err(e) => {
            tracing::error!(error = %e, "udf symbols task failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(UdfError::new("internal error")),
            )
                .into_response();
        }
    };
    if timeframes.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json(UdfError::new(format!("unknown symbol: {}", query.symbol))),
        )
            .into_response();
    }

    let resolutions = timeframes_to_resolutions(&timeframes);
    let info = SymbolInfo::from_symbol(&parsed, &resolutions, pricescale);
    (StatusCode::OK, Json(info)).into_response()
}

/// `/udf/search`：在本地数据目录里按交易对子串搜索。
pub(crate) async fn search_handler(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> impl IntoResponse {
    let markets: Vec<MarketType> = match query.exchange.as_deref() {
        None | Some("") => MarketType::SUPPORTED.to_vec(),
        Some(exchange) => match exchange.to_ascii_uppercase().as_str() {
            "BINANCE" | "BINANCEFUTURES" | "BINANCE_FUTURES" | "BINANCE-FUTURES" | "FUTURES" => {
                vec![MarketType::Futures]
            }
            "DUKASCOPY" | "FX" => vec![MarketType::Dukascopy],
            _ => return (StatusCode::OK, Json(Vec::new())),
        },
    };
    let needle = query.query.unwrap_or_default().trim().to_ascii_uppercase();
    let type_filter = query
        .symbol_type
        .as_deref()
        .map(str::to_ascii_lowercase)
        .filter(|t| !t.is_empty());
    let limit = query
        .limit
        .unwrap_or(DEFAULT_SEARCH_LIMIT)
        .clamp(1, MAX_SEARCH_LIMIT);

    let joined = tokio::task::spawn_blocking({
        let loader = state.data_loader.clone();
        move || {
            let mut out = Vec::new();
            for market in markets {
                for pair in loader.list_pairs(market)? {
                    if !needle.is_empty() && !pair.contains(&needle) {
                        continue;
                    }
                    let symbol_type = match market {
                        MarketType::Dukascopy => "forex",
                        MarketType::Futures | MarketType::Spot => "crypto",
                    };
                    if type_filter.as_deref().is_some_and(|t| t != symbol_type) {
                        continue;
                    }
                    let parsed = ParsedSymbol {
                        market,
                        pair: pair.clone(),
                    };
                    out.push(SearchItem::from_symbol(&parsed));
                    if out.len() >= limit {
                        return Ok::<Vec<SearchItem>, DataError>(out);
                    }
                }
            }
            Ok(out)
        }
    })
    .await;

    match joined {
        Ok(Ok(results)) => (StatusCode::OK, Json(results)),
        Ok(Err(e)) => {
            tracing::error!(error = %e, "udf search failed");
            (StatusCode::OK, Json(Vec::new()))
        }
        Err(e) => {
            tracing::error!(error = %e, "udf search task failed");
            (StatusCode::OK, Json(Vec::new()))
        }
    }
}
