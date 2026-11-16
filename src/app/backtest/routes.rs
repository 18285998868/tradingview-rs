//! 回测 HTTP 路由。
//!
//! 路径前缀 `/api/backtests`。所有响应沿用统一 [`ApiResponse`] 形状；CSV 导出
//! 例外，直接返回 `text/csv` 附件。

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;

use crate::app::routes::{ApiResponse, bad_request, handle_state_error, not_found};

use super::model::{
    BACKTEST_TIMEFRAMES, BacktestError, CompareRequest, CreateBacktestRequest, STRATEGY_DUAL_MA,
    STRATEGY_VERSION, SubmitOutcome,
};

/// 回测路由（状态类型为 [`crate::app::routes::AppState`]）。
pub(crate) fn router() -> Router<crate::app::routes::AppState> {
    Router::new()
        .route("/api/backtests/config", get(config_handler))
        .route("/api/backtests", get(list_handler).post(submit_handler))
        .route("/api/backtests/compare", post(compare_handler))
        .route("/api/backtests/compare.csv", get(compare_csv_handler))
        .route(
            "/api/backtests/{id}",
            get(detail_handler).delete(delete_handler),
        )
        .route("/api/backtests/{id}/log", get(log_handler))
        .route("/api/backtests/{id}/retry", post(retry_handler))
}

/// 取回测服务；配置关闭时返回 503。
fn disabled_response() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(ApiResponse::<serde_json::Value>::err(
            "backtest is disabled in config",
        )),
    )
        .into_response()
}

/// 回测错误 → HTTP 响应。
pub(crate) fn handle_backtest_error(error: BacktestError) -> Response {
    match error {
        BacktestError::Invalid(message) => bad_request(message).into_response(),
        BacktestError::NotFound => not_found("backtest run not found").into_response(),
        BacktestError::Disabled => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiResponse::<serde_json::Value>::err(
                "backtest is disabled in config",
            )),
        )
            .into_response(),
        BacktestError::Worker(message) => {
            tracing::error!(error = %message, "backtest worker failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<serde_json::Value>::err(
                    "backtest worker failed",
                )),
            )
                .into_response()
        }
        BacktestError::State(error) => handle_state_error(error).into_response(),
    }
}

/// 运行 ID 只允许 UUID / 简单标识符字符。
fn valid_run_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// 列表查询参数。
#[derive(Debug, Deserialize)]
struct ListQuery {
    limit: Option<usize>,
}

/// 日志查询参数。
#[derive(Debug, Deserialize)]
struct LogQuery {
    max_bytes: Option<usize>,
}

/// CSV 查询参数：逗号分隔的运行 ID。
#[derive(Debug, Deserialize)]
struct CompareCsvQuery {
    ids: String,
}

const LIST_LIMIT_DEFAULT: usize = 50;
const LIST_LIMIT_MAX: usize = 200;
const LOG_BYTES_DEFAULT: usize = 65_536;
const LOG_BYTES_MAX: usize = 1_048_576;

/// `GET /api/backtests/config`：前端初始化所需的策略、周期与上限。
async fn config_handler(State(state): State<crate::app::routes::AppState>) -> Response {
    let Some(service) = state.backtest.as_ref() else {
        return (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "enabled": false }))),
        )
            .into_response();
    };
    let config = service.config();
    (
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({
            "enabled": config.enabled,
            "market_type": "spot",
            "strategy": STRATEGY_DUAL_MA,
            "strategy_version": STRATEGY_VERSION,
            "direction": "long",
            "timeframes": BACKTEST_TIMEFRAMES,
            "defaults": {
                "fast": 10,
                "slow": 30,
                "fee_bps": 10.0,
                "slippage_bps": 5.0,
                "initial_balance": 10_000.0,
            },
            "limits": {
                "max_compare_runs": config.max_compare_runs,
                "max_grid_runs": config.max_grid_runs,
                "max_curve_points": config.max_curve_points,
                "max_trades": config.max_trades,
                "keep_equity": config.keep_equity,
            },
        }))),
    )
        .into_response()
}

/// `POST /api/backtests`：提交单组或参数扫描，返回批次与入队运行。
async fn submit_handler(
    State(state): State<crate::app::routes::AppState>,
    Json(request): Json<CreateBacktestRequest>,
) -> Response {
    let Some(service) = state.backtest.clone() else {
        return disabled_response();
    };
    match service.submit(request).await {
        Ok(outcome) => (StatusCode::ACCEPTED, Json(ApiResponse::ok(outcome))).into_response(),
        Err(error) => handle_backtest_error(error),
    }
}

/// `GET /api/backtests`：最近运行列表。
async fn list_handler(
    State(state): State<crate::app::routes::AppState>,
    Query(query): Query<ListQuery>,
) -> Response {
    let Some(service) = state.backtest.clone() else {
        return disabled_response();
    };
    let limit = query
        .limit
        .unwrap_or(LIST_LIMIT_DEFAULT)
        .clamp(1, LIST_LIMIT_MAX);
    match service.list(limit).await {
        Ok(runs) => (StatusCode::OK, Json(ApiResponse::ok(runs))).into_response(),
        Err(error) => handle_backtest_error(error),
    }
}

/// `GET /api/backtests/{id}`：运行详情（含交易、事件与曲线）。
async fn detail_handler(
    State(state): State<crate::app::routes::AppState>,
    Path(id): Path<String>,
) -> Response {
    let Some(service) = state.backtest.clone() else {
        return disabled_response();
    };
    if !valid_run_id(&id) {
        return bad_request("invalid backtest run id").into_response();
    }
    match service.detail(&id).await {
        Ok(detail) => (StatusCode::OK, Json(ApiResponse::ok(detail))).into_response(),
        Err(error) => handle_backtest_error(error),
    }
}

/// `DELETE /api/backtests/{id}`：删除运行及其明细。
async fn delete_handler(
    State(state): State<crate::app::routes::AppState>,
    Path(id): Path<String>,
) -> Response {
    let Some(service) = state.backtest.clone() else {
        return disabled_response();
    };
    if !valid_run_id(&id) {
        return bad_request("invalid backtest run id").into_response();
    }
    match service.delete(&id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_backtest_error(error),
    }
}

/// `GET /api/backtests/{id}/log`：失败日志尾部。
async fn log_handler(
    State(state): State<crate::app::routes::AppState>,
    Path(id): Path<String>,
    Query(query): Query<LogQuery>,
) -> Response {
    let Some(service) = state.backtest.clone() else {
        return disabled_response();
    };
    if !valid_run_id(&id) {
        return bad_request("invalid backtest run id").into_response();
    }
    let max_bytes = query
        .max_bytes
        .unwrap_or(LOG_BYTES_DEFAULT)
        .clamp(1_024, LOG_BYTES_MAX);
    match service.log(&id, max_bytes).await {
        Ok(Some(text)) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "log": text }))),
        )
            .into_response(),
        Ok(None) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "log": null }))),
        )
            .into_response(),
        Err(error) => handle_backtest_error(error),
    }
}

/// `POST /api/backtests/{id}/retry`：以原参数创建新运行。
async fn retry_handler(
    State(state): State<crate::app::routes::AppState>,
    Path(id): Path<String>,
) -> Response {
    let Some(service) = state.backtest.clone() else {
        return disabled_response();
    };
    if !valid_run_id(&id) {
        return bad_request("invalid backtest run id").into_response();
    }
    match service.retry(&id).await {
        Ok(SubmitOutcome { batch_id, runs }) => (
            StatusCode::ACCEPTED,
            Json(ApiResponse::ok(serde_json::json!({
                "batch_id": batch_id,
                "runs": runs,
            }))),
        )
            .into_response(),
        Err(error) => handle_backtest_error(error),
    }
}

/// `POST /api/backtests/compare`：2..=上限个成功运行的对比。
async fn compare_handler(
    State(state): State<crate::app::routes::AppState>,
    Json(request): Json<CompareRequest>,
) -> Response {
    let Some(service) = state.backtest.clone() else {
        return disabled_response();
    };
    match service.compare(&request.ids).await {
        Ok(response) => (StatusCode::OK, Json(ApiResponse::ok(response))).into_response(),
        Err(error) => handle_backtest_error(error),
    }
}

/// `GET /api/backtests/compare.csv?ids=a,b`：对比指标 CSV。
async fn compare_csv_handler(
    State(state): State<crate::app::routes::AppState>,
    Query(query): Query<CompareCsvQuery>,
) -> Response {
    let Some(service) = state.backtest.clone() else {
        return disabled_response();
    };
    let ids = query
        .ids
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    match service.compare_csv(&ids).await {
        Ok(csv) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "text/csv; charset=utf-8"),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=\"backtest-compare.csv\"",
                ),
            ],
            Body::from(csv),
        )
            .into_response(),
        Err(error) => handle_backtest_error(error),
    }
}
