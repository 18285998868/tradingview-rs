//! 响应压缩：`Accept-Encoding` 协商、gzip 内容可还原、小响应不压。
//!
//! 生产组装在 `app::run`（压缩是 `fallback_service` 之后的最外层），这里用
//! 同样的顺序（路由 → 压缩）验证响应行为。

use std::io::Read as _;
use std::path::Path;

use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, Request, StatusCode, header};
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;
use tower_http::compression::CompressionLayer;
use tradingview_backend::app::AppState;
use tradingview_backend::app::routes::create_router;
use tradingview_backend::app::state::StateStore;

/// 2024-01-01T00:00:00Z。
const BASE_MS: i64 = 1_704_067_200_000;
const MINUTE_MS: i64 = 60_000;

/// 写一份 binance futures zip；`rows` 必须落在文件名的 UTC 日期内。
fn write_zip(dir: &Path, pair: &str, timeframe: &str, step_ms: i64, rows: usize) {
    let tf_dir = dir
        .join("binance")
        .join("data")
        .join("futures")
        .join("um")
        .join("daily")
        .join("klines")
        .join(pair)
        .join(timeframe);
    std::fs::create_dir_all(&tf_dir).unwrap();

    let mut csv = String::new();
    for i in 0..rows {
        let open_time = BASE_MS + i as i64 * step_ms;
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},0,0,0,0,0\n",
            open_time,
            100.0 + i as f64,
            101.0 + i as f64,
            99.0 + i as f64,
            100.5 + i as f64,
            10.0 + i as f64,
            open_time + step_ms - 1
        ));
    }

    let name = format!("{pair}-{timeframe}-2024-01-01");
    let file = std::fs::File::create(tf_dir.join(format!("{name}.zip"))).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file::<&str, ()>(&format!("{name}.csv"), zip::write::FileOptions::default())
        .unwrap();
    use std::io::Write as _;
    zip.write_all(csv.as_bytes()).unwrap();
    zip.finish().unwrap();
}

fn app(dir: &Path) -> axum::Router {
    let store = std::sync::Arc::new(StateStore::open_in_memory().unwrap());
    create_router(AppState::new(dir, store)).layer(CompressionLayer::new())
}

async fn request(
    app: &axum::Router,
    uri: &str,
    accept_encoding: Option<&str>,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut builder = Request::builder().uri(uri);
    if let Some(accept) = accept_encoding {
        builder = builder.header(header::ACCEPT_ENCODING, accept);
    }
    let res = app
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    (status, headers, body.to_vec())
}

fn gunzip(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(bytes)
        .read_to_end(&mut out)
        .unwrap();
    out
}

#[tokio::test]
async fn gzip_roundtrip_matches_identity_body() {
    let dir = TempDir::new().unwrap();
    write_zip(dir.path(), "BTCUSDT", "1m", MINUTE_MS, 600);
    let app = app(dir.path());
    let uri = "/api/klines?pair=BTCUSDT&timeframe=1m";

    let (status, plain_headers, plain) = request(&app, uri, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(plain_headers.get(header::CONTENT_ENCODING).is_none());

    let (status, gz_headers, gz) = request(&app, uri, Some("gzip")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        gz_headers[header::CONTENT_ENCODING].to_str().unwrap(),
        "gzip"
    );
    assert!(
        gz.len() < plain.len(),
        "gzip {} 字节不小于原文 {} 字节",
        gz.len(),
        plain.len()
    );
    // 字节级相等：压缩不能改变 JSON 内容与字段顺序。
    assert_eq!(gunzip(&gz), plain);
}

#[tokio::test]
async fn small_response_is_not_compressed() {
    // `/udf/time` 只有二十来字节，低于 tower-http 默认的 32 字节阈值。
    let dir = TempDir::new().unwrap();
    let app = app(dir.path());

    let (status, headers, body) = request(&app, "/udf/time", Some("gzip")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.get(header::CONTENT_ENCODING).is_none());
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert!(value["time"].as_i64().unwrap() > 0);
}

#[tokio::test]
async fn identity_encoding_is_respected() {
    let dir = TempDir::new().unwrap();
    write_zip(dir.path(), "BTCUSDT", "1m", MINUTE_MS, 600);
    let app = app(dir.path());

    let (status, headers, body) =
        request(&app, "/api/pairs?market_type=futures", Some("identity")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.get(header::CONTENT_ENCODING).is_none());
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["data"]["pairs"], Value::from(vec!["BTCUSDT"]));
}
