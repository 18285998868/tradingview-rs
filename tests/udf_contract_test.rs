//! UDF 协议契约测试。
//!
//! 目的：逐字段锁定 `/udf/*` 的响应形状与官方语义（`to` 右开、`countback`
//! 优先、`no_data.nextTime`、按数据推导 `pricescale`）。修改协议行为时必须
//! 同步修改这里的断言。

use std::path::Path;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;
use tradingview_backend::app::AppState;
use tradingview_backend::app::routes::create_router;
use tradingview_backend::app::state::StateStore;

/// 写入一份 binance futures zip；`period` 是 `daily` / `monthly`，
/// `step_ms` 用于校验 bar 的 `close_time`。
fn write_zip(
    dir: &Path,
    period: &str,
    pair: &str,
    timeframe: &str,
    step_ms: i64,
    rows: &[(i64, f64, f64, f64, f64, f64)],
) {
    let tf_dir = dir
        .join("binance")
        .join("data")
        .join("futures")
        .join("um")
        .join(period)
        .join("klines")
        .join(pair)
        .join(timeframe);
    std::fs::create_dir_all(&tf_dir).unwrap();
    let mut csv = String::new();
    for r in rows {
        // reader 校验 close_time = open + step - 1ms。
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},0,0,0,0,0\n",
            r.0,
            r.1,
            r.2,
            r.3,
            r.4,
            r.5,
            r.0 + step_ms - 1
        ));
    }
    let name = match period {
        "monthly" => format!("{pair}-{timeframe}-2023-11.zip"),
        _ => format!("{pair}-{timeframe}-2023-11-14.zip"),
    };
    let file = std::fs::File::create(tf_dir.join(&name)).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file::<&str, ()>(
        &name.replace(".zip", ".csv"),
        zip::write::FileOptions::default(),
    )
    .unwrap();
    use std::io::Write as _;
    zip.write_all(csv.as_bytes()).unwrap();
    zip.finish().unwrap();
}

fn app(dir: &Path) -> axum::Router {
    let store = std::sync::Arc::new(StateStore::open_in_memory().unwrap());
    create_router(AppState::new(dir, store))
}

async fn get_json(app: &axum::Router, uri: &str) -> (StatusCode, Value) {
    let res = app
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

/// 三根 1h bar，覆盖 2023-11-14 00:00–03:00 UTC。
const BASE_MS: i64 = 1_699_920_000_000;
const HOUR_MS: i64 = 3_600_000;
const BTC_ROWS: [(i64, f64, f64, f64, f64, f64); 3] = [
    (BASE_MS, 100.0, 105.0, 95.0, 102.0, 10.0),
    (BASE_MS + 3_600_000, 102.0, 107.0, 97.0, 104.0, 20.0),
    (BASE_MS + 7_200_000, 104.0, 108.0, 100.0, 101.0, 30.0),
];

fn btc_app() -> (TempDir, axum::Router) {
    let dir = TempDir::new().unwrap();
    write_zip(dir.path(), "daily", "BTCUSDT", "1h", HOUR_MS, &BTC_ROWS);
    let app = app(dir.path());
    (dir, app)
}

#[tokio::test]
async fn contract_config_shape() {
    let (_dir, app) = btc_app();
    let (status, config) = get_json(&app, "/udf/config").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        config,
        json!({
            "supported_resolutions": ["1", "3", "5", "15", "30", "60", "120", "240", "360", "480", "720", "1D", "3D", "1W", "1M"],
            "supports_search": true,
            "supports_time": true,
            "supports_marks": false,
            "supports_timescale_marks": false,
            "supports_group_request": false,
            "supports_news": false,
            "symbols_types": [
                { "name": "Crypto", "value": "crypto" },
                { "name": "Forex", "value": "forex" }
            ],
            "exchanges": [
                { "value": "BINANCE", "name": "Binance Futures", "desc": "Binance UM futures" },
                { "value": "DUKASCOPY", "name": "Dukascopy", "desc": "Dukascopy FX / metals" }
            ]
        })
    );
}

#[tokio::test]
async fn contract_symbol_info_shape() {
    let (_dir, app) = btc_app();
    let (status, symbol) = get_json(&app, "/udf/symbols?symbol=BINANCE:BTCUSDT").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        symbol,
        json!({
            "name": "BTCUSDT",
            "full_name": "BINANCE:BTCUSDT",
            "ticker": "BTCUSDT",
            "description": "BTCUSDT · Binance Futures",
            "type": "crypto",
            "exchange": "BINANCE",
            "listed_exchange": "BINANCE",
            "session": "24x7",
            "timezone": "Etc/UTC",
            "format": "price",
            "minmov": 1,
            "pricescale": 100,
            "has_intraday": true,
            "has_daily": false,
            "has_weekly_and_monthly": false,
            "has_empty_bars": false,
            "supported_resolutions": ["60"]
        })
    );
}

#[tokio::test]
async fn contract_weekly_symbol_shape() {
    let dir = TempDir::new().unwrap();
    write_zip(dir.path(), "daily", "BTCUSDT", "1h", HOUR_MS, &BTC_ROWS);
    const WEEK_MS: i64 = 7 * 86_400_000;
    write_zip(
        dir.path(),
        "monthly",
        "BTCUSDT",
        "1w",
        WEEK_MS,
        &[(BASE_MS, 100.0, 120.0, 90.0, 110.0, 700.0)],
    );
    let app = app(dir.path());
    let (status, symbol) = get_json(&app, "/udf/symbols?symbol=BINANCE:BTCUSDT").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(symbol["has_weekly_and_monthly"], true);
    assert_eq!(symbol["weekly_multipliers"], json!(["1"]));
    assert!(
        symbol["supported_resolutions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "1W")
    );
    assert!(
        symbol.get("monthly_multipliers").is_none(),
        "monthly multipliers should only appear when monthly data exists"
    );

    let (status, history) = get_json(
        &app,
        "/udf/history?symbol=BINANCE:BTCUSDT&resolution=1W&from=1699920000&to=1700524800",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(history["t"], json!([1_699_920_000_i64]));
}

#[tokio::test]
async fn contract_search_shape() {
    let (_dir, app) = btc_app();
    let (status, search) = get_json(&app, "/udf/search?query=btc&limit=5").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        search,
        json!([{
            "symbol": "BINANCE:BTCUSDT",
            "full_name": "BINANCE:BTCUSDT",
            "description": "BTCUSDT · Binance Futures",
            "exchange": "BINANCE",
            "ticker": "BTCUSDT",
            "type": "crypto"
        }])
    );

    let (status, filtered) = get_json(&app, "/udf/search?query=BTC&type=forex").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(filtered, json!([]));
}

#[tokio::test]
async fn contract_history_ok_shape() {
    let (_dir, app) = btc_app();
    let (status, history) = get_json(
        &app,
        "/udf/history?symbol=BINANCE:BTCUSDT&resolution=60&from=1699920000&to=1699930800",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        history,
        json!({
            "s": "ok",
            "t": [1_699_920_000_i64, 1_699_923_600_i64, 1_699_927_200_i64],
            "o": [100.0, 102.0, 104.0],
            "h": [105.0, 107.0, 108.0],
            "l": [95.0, 97.0, 100.0],
            "c": [102.0, 104.0, 101.0],
            "v": [10.0, 20.0, 30.0]
        })
    );
}

#[tokio::test]
async fn contract_history_no_data_and_error_shape() {
    let (_dir, app) = btc_app();

    let (status, no_data) = get_json(
        &app,
        "/udf/history?symbol=BINANCE:NOPE&resolution=60&from=0&to=0",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(no_data, json!({ "s": "no_data" }));

    let (status, bad_resolution) = get_json(
        &app,
        "/udf/history?symbol=BINANCE:BTCUSDT&resolution=2D&from=0&to=0",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        bad_resolution,
        json!({ "s": "error", "errmsg": "unsupported resolution: 2D" })
    );

    let (status, negative) = get_json(
        &app,
        "/udf/history?symbol=BINANCE:BTCUSDT&resolution=60&from=-1&to=0",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        negative,
        json!({ "s": "error", "errmsg": "from/to must be >= 0" })
    );
}

/// 低价币的 `pricescale` 由样本价格的实际小数位推导（DOGE 常见 tick 0.00001）。
#[tokio::test]
async fn pricescale_reflects_quote_precision() {
    let dir = TempDir::new().unwrap();
    write_zip(
        dir.path(),
        "daily",
        "DOGEUSDT",
        "1h",
        HOUR_MS,
        &[
            (BASE_MS, 0.123_45, 0.123_47, 0.123_44, 0.123_46, 1_000.0),
            (
                BASE_MS + 3_600_000,
                0.123_46,
                0.123_48,
                0.123_45,
                0.123_47,
                2_000.0,
            ),
            (
                BASE_MS + 7_200_000,
                0.123_47,
                0.123_49,
                0.123_46,
                0.123_48,
                3_000.0,
            ),
        ],
    );
    let app = app(dir.path());
    let (status, symbol) = get_json(&app, "/udf/symbols?symbol=BINANCE:DOGEUSDT").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        symbol["pricescale"], 100_000,
        "pricescale should follow the 5 decimals present in the sample bars"
    );
}

/// 官方 UDF 规定 `to` 为右开区间。
#[tokio::test]
async fn to_is_exclusive() {
    let (_dir, app) = btc_app();
    let (status, history) = get_json(
        &app,
        "/udf/history?symbol=BINANCE:BTCUSDT&resolution=60&from=1699920000&to=1699923600",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        history["t"],
        json!([1_699_920_000_i64]),
        "to=1699923600 must exclude the bar opened at 1699923600"
    );
}

/// 官方 UDF 规定 `countback` 优先于 `from`，且从 `to` 往回数。
#[tokio::test]
async fn countback_overrides_from() {
    let (_dir, app) = btc_app();
    let (status, history) = get_json(
        &app,
        "/udf/history?symbol=BINANCE:BTCUSDT&resolution=60&from=1699927200&to=1699927200&countback=2",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        history["t"],
        json!([1_699_920_000_i64, 1_699_923_600_i64]),
        "from must be ignored when countback is present"
    );
}

/// 官方 UDF 用 `nextTime` 告知过去最近的可用 bar。
#[tokio::test]
async fn no_data_reports_next_time() {
    let (_dir, app) = btc_app();
    let (status, no_data) = get_json(
        &app,
        "/udf/history?symbol=BINANCE:BTCUSDT&resolution=60&from=1700000000&to=1700003600",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        no_data,
        json!({ "s": "no_data", "nextTime": 1_699_927_200_i64 })
    );
}
