//! 回测 API 集成测试：用一个假 Python worker 驱动完整状态机。
//!
//! 真 worker 的数值正确性由 `python/tests/` 覆盖；这里只验证 Rust 侧的契约：
//! 参数展开、状态流转、持久化、对比上限/警告、CSV、失败日志、重试与重启恢复。

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde::Deserialize;
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

use tradingview_backend::app::backtest::model::{
    BacktestRunDetail, BacktestRunSummary, CompareResponse, RunStatus, SubmitOutcome,
};
use tradingview_backend::app::backtest::service::BacktestService;
use tradingview_backend::app::routes::{AppState, create_router};
use tradingview_backend::app::state::StateStore;
use tradingview_backend::config::BacktestConfig;

const FAKE_WORKER: &str = r#"#!/usr/bin/env python3
import json
import sys

args = sys.argv[1:]
request_path = args[args.index("--request") + 1]
output_path = args[args.index("--output") + 1]
with open(request_path, encoding="utf-8") as handle:
    request = json.load(handle)

metrics = {
    "num_trades": 1,
    "win_rate": 100.0,
    "total_return_pct": 1.0,
    "sharpe_ratio": 1.5,
    "sortino_ratio": 2.0,
    "max_drawdown_pct": 1.0,
    "avg_drawdown_pct": 1.0,
    "drawdown_count": 1,
    "longest_drawdown_days": 0.5,
    "longest_recovery_days": 0.25,
    "drawdown_time_pct": 10.0,
    "profit_factor": None,
    "expectancy": 0.01,
    "final_equity": 10100.0,
    "bars": 10,
    "periods_per_year": 8760.0,
}
runs = []
for params in request["params_list"]:
    runs.append({
        "params": params,
        "params_text": "fast=%d,slow=%d" % (params["fast"], params["slow"]),
        "bars": 10,
        "first_time": 1000,
        "last_time": 9000,
        "metrics": metrics,
        "trades": [{
            "seq": 0,
            "entry_time": 2000,
            "exit_time": 3000,
            "side": "long",
            "status": "closed",
            "entry_price": 100.0,
            "exit_price": 101.0,
            "qty": 1.0,
            "pnl": 1.0,
            "pnl_pct": 1.0,
            "bars_held": 1,
        }],
        "trades_truncated": False,
        "drawdown_events": [{
            "seq": 0,
            "peak_time": 2000,
            "trough_time": 3000,
            "recovery_time": 4000,
            "depth_pct": 1.0,
            "duration_ms": 2000,
            "recovery_ms": 1000,
            "recovered": True,
        }],
        "equity": [[1000, 10000.0], [9000, 10100.0]],
        "underwater": [[1000, 0.0], [5000, -1.0], [9000, 0.0]],
        "error": None,
    })

payload = {
    "worker_version": "test",
    "engine": "fake vectorbt",
    "strategy": "dual_ma",
    "strategy_version": "1",
    "pair": request["pair"],
    "timeframe": request["timeframe"],
    "fee_bps": request["fee_bps"],
    "slippage_bps": request["slippage_bps"],
    "initial_balance": request["initial_balance"],
    "start_ms": request.get("start_ms"),
    "end_ms": request.get("end_ms"),
    "files": 1,
    "rows": 10,
    "first_time": 1000,
    "last_time": 9000,
    "runs": runs,
}
with open(output_path, "w", encoding="utf-8") as handle:
    json.dump(payload, handle)
"#;

const FAIL_WORKER: &str = r#"#!/bin/sh
echo "boom from fake worker" >&2
exit 3
"#;

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    success: bool,
    data: Option<T>,
    error: Option<String>,
}

struct Harness {
    app: Router,
    service: Arc<BacktestService>,
    _store: Arc<StateStore>,
    _dir: TempDir,
}

fn write_script(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).unwrap();
    }
}

fn harness(worker_body: &str) -> Harness {
    let dir = TempDir::new().unwrap();
    let worker_dir = dir.path().join("python");
    let data_dir = dir.path().join("data");
    let output_dir = dir.path().join("backtests");
    std::fs::create_dir_all(&worker_dir).unwrap();
    std::fs::create_dir_all(&data_dir).unwrap();
    let script = worker_dir.join("fake_worker.py");
    write_script(&script, worker_body);

    let db_path = dir.path().join("test.db");
    let store = Arc::new(StateStore::open(&db_path).unwrap());
    let config = BacktestConfig {
        enabled: true,
        python_bin: script,
        worker_dir: worker_dir.clone(),
        output_dir: Some(output_dir),
        timeout_secs: 20,
        max_concurrent: 2,
        max_compare_runs: 10,
        max_grid_runs: 100,
        max_curve_points: 100,
        max_trades: 1_000,
        keep_equity: true,
    };
    let service = Arc::new(BacktestService::new(Arc::clone(&store), &data_dir, config).unwrap());
    let app = create_router(
        AppState::new(data_dir, Arc::clone(&store)).with_backtest(Arc::clone(&service)),
    );
    Harness {
        app,
        service,
        _store: store,
        _dir: dir,
    }
}

async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Vec<u8>) {
    let builder = Request::builder().method(method).uri(uri);
    let request = match body {
        Some(value) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(value.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    (status, bytes.to_vec())
}

fn parse<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Envelope<T> {
    serde_json::from_slice(bytes).unwrap()
}

async fn wait_for(service: &Arc<BacktestService>, id: &str, want: RunStatus) -> BacktestRunSummary {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let detail = service.detail(id).await.unwrap();
        if detail.run.status == want {
            return detail.run;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "run {id} stuck at {:?}, waiting for {want:?}",
            detail.run.status
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn request_body() -> Value {
    json!({
        "pair": "BTCUSDT",
        "timeframe": "1h",
        "params": { "fast": 10, "slow": 30 },
        "fee_bps": 10.0,
        "slippage_bps": 5.0,
        "initial_balance": 10000.0
    })
}

#[tokio::test]
async fn submit_success_persists_detail_and_markers() {
    let harness = harness(FAKE_WORKER);
    let (status, body) = call(&harness.app, "POST", "/api/backtests", Some(request_body())).await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let envelope = parse::<SubmitOutcome>(&body);
    assert!(envelope.success);
    let outcome = envelope.data.unwrap();
    assert_eq!(outcome.runs.len(), 1);
    assert_eq!(outcome.runs[0].status, RunStatus::Queued);
    let id = outcome.runs[0].id.clone();

    let run = wait_for(&harness.service, &id, RunStatus::Succeeded).await;
    assert_eq!(run.metrics.as_ref().unwrap().num_trades, 1);
    assert_eq!(run.engine.as_deref(), Some("fake vectorbt"));
    assert_eq!(run.bars, Some(10));

    let (status, body) = call(&harness.app, "GET", &format!("/api/backtests/{id}"), None).await;
    assert_eq!(status, StatusCode::OK);
    let detail = parse::<BacktestRunDetail>(&body).data.unwrap();
    assert_eq!(detail.trades.len(), 1);
    assert_eq!(detail.drawdown_events.len(), 1);
    assert_eq!(detail.equity.len(), 2);
    assert_eq!(detail.underwater.len(), 3);
    assert_eq!(detail.markers.len(), 2);
    assert_eq!(detail.markers[0].kind, "entry");
    assert_eq!(detail.markers[1].kind, "exit");

    let (status, body) = call(&harness.app, "GET", "/api/backtests?limit=10", None).await;
    assert_eq!(status, StatusCode::OK);
    let runs = parse::<Vec<BacktestRunSummary>>(&body).data.unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].id, id);
}

#[tokio::test]
async fn short_direction_is_rejected_with_readable_error() {
    let harness = harness(FAKE_WORKER);
    let mut body = request_body();
    body["direction"] = json!("short");
    let (status, bytes) = call(&harness.app, "POST", "/api/backtests", Some(body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let envelope = parse::<Value>(&bytes);
    assert!(!envelope.success);
    assert!(envelope.error.unwrap().contains("direction must be long"));
}

#[tokio::test]
async fn param_grid_creates_one_batch_and_compare_exports_csv() {
    let harness = harness(FAKE_WORKER);
    let mut body = request_body();
    body["param_grid"] = json!({ "fast": [5, 10], "slow": [20, 30] });
    let (status, bytes) = call(&harness.app, "POST", "/api/backtests", Some(body)).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let outcome = parse::<SubmitOutcome>(&bytes).data.unwrap();
    assert_eq!(outcome.runs.len(), 4);
    assert!(
        outcome
            .runs
            .iter()
            .all(|run| run.batch_id == outcome.batch_id)
    );
    let mut ids = Vec::new();
    for run in &outcome.runs {
        let done = wait_for(&harness.service, &run.id, RunStatus::Succeeded).await;
        ids.push(done.id);
    }

    let (status, bytes) = call(
        &harness.app,
        "POST",
        "/api/backtests/compare",
        Some(json!({ "ids": [ids[0], ids[1]] })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let compare = parse::<CompareResponse>(&bytes).data.unwrap();
    assert!(compare.warnings.is_empty());
    assert_eq!(compare.series.len(), 2);
    assert_eq!(compare.union_points, 3);
    assert_eq!(compare.series[0].missing_points, 0);
    assert!(compare.series[0].label.contains("fast="));

    let uri = format!("/api/backtests/compare.csv?ids={},{}", ids[0], ids[1]);
    let (status, bytes) = call(&harness.app, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK);
    let csv = String::from_utf8(bytes).unwrap();
    let lines = csv.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].starts_with("run_id,label,pair,timeframe,strategy,"));
    assert!(csv.contains("BTCUSDT"));

    let (status, _) = call(
        &harness.app,
        "DELETE",
        &format!("/api/backtests/{}", ids[0]),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call(
        &harness.app,
        "GET",
        &format!("/api/backtests/{}", ids[0]),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn compare_rejects_too_many_and_duplicate_ids() {
    let harness = harness(FAKE_WORKER);
    let eleven = (0..11)
        .map(|index| format!("00000000-0000-0000-0000-{index:012}"))
        .collect::<Vec<_>>();
    let (status, bytes) = call(
        &harness.app,
        "POST",
        "/api/backtests/compare",
        Some(json!({ "ids": eleven })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(parse::<Value>(&bytes).error.unwrap().contains("at most 10"));

    let duplicate = vec!["aaaaaaaa-0000-0000-0000-000000000000"; 2];
    let (status, bytes) = call(
        &harness.app,
        "POST",
        "/api/backtests/compare",
        Some(json!({ "ids": duplicate })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(parse::<Value>(&bytes).error.unwrap().contains("duplicate"));
}

#[tokio::test]
async fn failure_exposes_log_and_retry_creates_new_run() {
    let harness = harness(FAIL_WORKER);
    let (status, bytes) = call(&harness.app, "POST", "/api/backtests", Some(request_body())).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let outcome = parse::<SubmitOutcome>(&bytes).data.unwrap();
    let original = outcome.runs[0].id.clone();
    let failed = wait_for(&harness.service, &original, RunStatus::Failed).await;
    assert!(failed.error.unwrap().contains("boom from fake worker"));

    let (status, bytes) = call(
        &harness.app,
        "GET",
        &format!("/api/backtests/{original}/log"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let log = parse::<Value>(&bytes).data.unwrap();
    assert!(
        log["log"]
            .as_str()
            .unwrap()
            .contains("boom from fake worker")
    );

    let (status, bytes) = call(
        &harness.app,
        "POST",
        &format!("/api/backtests/{original}/retry"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let retried = parse::<Value>(&bytes).data.unwrap();
    let new_id = retried["runs"][0]["id"].as_str().unwrap().to_owned();
    assert_ne!(new_id, original);
    assert_ne!(retried["batch_id"].as_str().unwrap(), outcome.batch_id);
    wait_for(&harness.service, &new_id, RunStatus::Failed).await;

    let historical = harness.service.detail(&original).await.unwrap();
    assert_eq!(historical.run.status, RunStatus::Failed);
}

#[tokio::test]
async fn restart_marks_queued_and_running_as_failed() {
    let dir = TempDir::new().unwrap();
    let worker_dir = dir.path().join("python");
    let data_dir = dir.path().join("data");
    std::fs::create_dir_all(&worker_dir).unwrap();
    std::fs::create_dir_all(&data_dir).unwrap();
    let script = worker_dir.join("fake_worker.py");
    write_script(&script, FAKE_WORKER);
    let db_path = dir.path().join("restart.db");
    {
        let store = StateStore::open(&db_path).unwrap();
        drop(store);
    }
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        for (id, status) in [("queued-run", "queued"), ("running-run", "running")] {
            conn.execute(
                "INSERT INTO backtest_runs
                    (id, batch_id, status, market_type, pair, timeframe, strategy,
                     strategy_version, params_json, params_text, fee_bps, slippage_bps,
                     initial_balance, created_at)
                 VALUES (?1, 'batch', ?2, 'spot', 'BTCUSDT', '1h', 'dual_ma', '1',
                         '{\"fast\":10,\"slow\":30}', 'fast=10,slow=30', 10, 5, 10000, 1)",
                rusqlite::params![id, status],
            )
            .unwrap();
        }
    }

    let store = Arc::new(StateStore::open(&db_path).unwrap());
    let config = BacktestConfig {
        enabled: true,
        python_bin: script,
        worker_dir,
        output_dir: Some(dir.path().join("out")),
        timeout_secs: 20,
        max_concurrent: 2,
        max_compare_runs: 10,
        max_grid_runs: 100,
        max_curve_points: 100,
        max_trades: 1_000,
        keep_equity: true,
    };
    let service = BacktestService::new(Arc::clone(&store), &data_dir, config).unwrap();
    let runs = service.list(10).await.unwrap();
    assert_eq!(runs.len(), 2);
    for run in runs {
        assert_eq!(run.status, RunStatus::Failed);
        assert!(run.error.unwrap().contains("server restarted"));
    }
}

#[tokio::test]
async fn config_endpoint_reports_limits() {
    let harness = harness(FAKE_WORKER);
    let (status, bytes) = call(&harness.app, "GET", "/api/backtests/config", None).await;
    assert_eq!(status, StatusCode::OK);
    let config = parse::<Value>(&bytes).data.unwrap();
    assert_eq!(config["enabled"], json!(true));
    assert_eq!(config["strategy"], json!("dual_ma"));
    assert_eq!(config["limits"]["max_compare_runs"], json!(10));
    assert_eq!(config["timeframes"].as_array().unwrap().len(), 12);
}
