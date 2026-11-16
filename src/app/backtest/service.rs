//! 回测任务服务：提交、参数展开、Python 子进程、结果落库与重试。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::sync::Semaphore;

use crate::app::state::StateStore;
use crate::config::BacktestConfig;
use crate::error::AppError;

use super::compare::{build_compare, export_csv};
use super::model::{
    BacktestError, BacktestMetrics, BacktestRunDetail, BacktestRunSummary, BacktestTrade,
    CompareResponse, CreateBacktestRequest, DrawdownEvent, DualMaParams, EquityPoint,
    SubmitOutcome, UnderwaterPoint,
};
use super::store::{StoredBacktestResult, load_underwater};

/// 回测服务。
pub struct BacktestService {
    store: Arc<StateStore>,
    config: BacktestConfig,
    output_dir: PathBuf,
    worker_dir: PathBuf,
    python_bin: PathBuf,
    data_dir: PathBuf,
    semaphore: Arc<Semaphore>,
}

impl std::fmt::Debug for BacktestService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BacktestService")
            .field("output_dir", &self.output_dir)
            .field("worker_dir", &self.worker_dir)
            .field("python_bin", &self.python_bin)
            .field("max_concurrent", &self.config.max_concurrent)
            .finish_non_exhaustive()
    }
}

impl BacktestService {
    /// 构造服务：解析绝对路径、创建输出目录、回收中断的运行。
    ///
    /// # Errors
    /// 目录不可创建/不存在或状态库更新失败时返回 [`AppError`]。
    pub fn new(
        store: Arc<StateStore>,
        data_dir: &Path,
        config: BacktestConfig,
    ) -> Result<Self, AppError> {
        config.validate()?;
        let worker_dir = std::fs::canonicalize(&config.worker_dir).map_err(|source| {
            AppError::Config(format!(
                "backtest.worker_dir {} is not accessible: {source}",
                config.worker_dir.display()
            ))
        })?;
        if !worker_dir.is_dir() {
            return Err(AppError::Config(format!(
                "backtest.worker_dir is not a directory: {}",
                worker_dir.display()
            )));
        }
        let output_dir = config
            .output_dir
            .clone()
            .unwrap_or_else(|| data_dir.join("backtests"));
        std::fs::create_dir_all(&output_dir).map_err(|source| {
            AppError::Config(format!(
                "failed to create backtest.output_dir {}: {source}",
                output_dir.display()
            ))
        })?;
        let output_dir = std::fs::canonicalize(&output_dir)?;
        let data_dir = std::fs::canonicalize(data_dir)?;
        let python_bin = resolve_python_bin(&config.python_bin);
        let permits = config.max_concurrent;
        let service = Self {
            store,
            config,
            output_dir,
            worker_dir,
            python_bin,
            data_dir,
            semaphore: Arc::new(Semaphore::new(permits)),
        };
        let interrupted = service.store.mark_interrupted_backtests(now())?;
        if interrupted > 0 {
            tracing::warn!(interrupted, "marked interrupted backtest runs as failed");
        }
        Ok(service)
    }

    /// 配置上限（前端初始化用）。
    pub fn config(&self) -> &BacktestConfig {
        &self.config
    }

    /// 提交新回测；参数网格会展开成多个运行，共享一个批次。
    ///
    /// # Errors
    /// 请求非法、状态库失败时返回 [`BacktestError`]。
    pub async fn submit(
        self: &Arc<Self>,
        request: CreateBacktestRequest,
    ) -> Result<SubmitOutcome, BacktestError> {
        let plan = request.plan(&self.config)?;
        let batch_id = uuid::Uuid::new_v4().to_string();
        let runs = plan
            .params_list
            .iter()
            .map(|params| (uuid::Uuid::new_v4().to_string(), *params))
            .collect::<Vec<_>>();
        let store = Arc::clone(&self.store);
        let plan_for_store = plan.clone();
        let runs_for_store = runs.clone();
        let keep_equity = self.config.keep_equity;
        let created_at = now();
        let batch_for_store = batch_id.clone();
        let summaries = tokio::task::spawn_blocking(move || {
            store.create_backtest_runs(
                &batch_for_store,
                &plan_for_store,
                &runs_for_store,
                keep_equity,
                created_at,
            )
        })
        .await
        .map_err(|error| BacktestError::Worker(format!("state task join failed: {error}")))??;

        let service = Arc::clone(self);
        let batch_for_task = batch_id.clone();
        tokio::spawn(async move {
            service.execute_batch(batch_for_task, plan, runs).await;
        });

        Ok(SubmitOutcome {
            batch_id,
            runs: summaries,
        })
    }

    /// 用原参数重新提交一个新运行（保留历史 ID）。
    ///
    /// # Errors
    /// 运行不存在、未成功或状态库失败时返回 [`BacktestError`]。
    pub async fn retry(self: &Arc<Self>, id: &str) -> Result<SubmitOutcome, BacktestError> {
        let store = Arc::clone(&self.store);
        let id_owned = id.to_owned();
        let run = tokio::task::spawn_blocking(move || store.get_backtest_run(&id_owned))
            .await
            .map_err(|error| BacktestError::Worker(format!("state task join failed: {error}")))??
            .ok_or(BacktestError::NotFound)?;
        let request = CreateBacktestRequest {
            market_type: Some(run.market_type),
            pair: run.pair,
            timeframe: run.timeframe,
            strategy: Some(run.strategy),
            direction: Some("long".into()),
            params: Some(run.params),
            param_grid: None,
            fee_bps: run.fee_bps,
            slippage_bps: run.slippage_bps,
            initial_balance: Some(run.initial_balance),
            start_ms: run.start_ms,
            end_ms: run.end_ms,
        };
        self.submit(request).await
    }

    /// 运行列表。
    ///
    /// # Errors
    /// 状态库失败时返回 [`BacktestError`]。
    pub async fn list(&self, limit: usize) -> Result<Vec<BacktestRunSummary>, BacktestError> {
        let store = Arc::clone(&self.store);
        let runs = tokio::task::spawn_blocking(move || store.list_backtest_runs(limit))
            .await
            .map_err(|error| BacktestError::Worker(format!("state task join failed: {error}")))??;
        Ok(runs)
    }

    /// 运行详情。
    ///
    /// # Errors
    /// 运行不存在时返回 [`BacktestError::NotFound`]；状态库失败时返回
    /// [`BacktestError::State`]。
    pub async fn detail(&self, id: &str) -> Result<BacktestRunDetail, BacktestError> {
        let store = Arc::clone(&self.store);
        let id_owned = id.to_owned();
        let detail = tokio::task::spawn_blocking(move || store.get_backtest_detail(&id_owned))
            .await
            .map_err(|error| BacktestError::Worker(format!("state task join failed: {error}")))??
            .ok_or(BacktestError::NotFound)?;
        Ok(detail)
    }

    /// 删除运行。
    ///
    /// # Errors
    /// 运行不存在时返回 [`BacktestError::NotFound`]。
    pub async fn delete(&self, id: &str) -> Result<(), BacktestError> {
        let store = Arc::clone(&self.store);
        let id_owned = id.to_owned();
        let deleted = tokio::task::spawn_blocking(move || store.delete_backtest(&id_owned))
            .await
            .map_err(|error| BacktestError::Worker(format!("state task join failed: {error}")))??;
        if deleted {
            Ok(())
        } else {
            Err(BacktestError::NotFound)
        }
    }

    /// 读取失败日志尾部；运行不存在或没有日志返回 `None`。
    ///
    /// # Errors
    /// 运行不存在时返回 [`BacktestError::NotFound`]。
    pub async fn log(&self, id: &str, max_bytes: usize) -> Result<Option<String>, BacktestError> {
        let run = self.detail(id).await?.run;
        let Some(path) = run.log_path else {
            return Ok(None);
        };
        let path = PathBuf::from(path);
        if !path.starts_with(&self.output_dir) {
            tracing::warn!(path = %path.display(), "ignoring log outside output dir");
            return Ok(None);
        }
        let bytes = tokio::fs::read(&path).await.map_err(|error| {
            BacktestError::Worker(format!("failed to read log {}: {error}", path.display()))
        })?;
        let start = bytes.len().saturating_sub(max_bytes);
        let text = String::from_utf8_lossy(&bytes[start..]).into_owned();
        Ok(Some(text))
    }

    /// 对比 2..=上限个成功运行。
    ///
    /// # Errors
    /// 数量越界、ID 重复/不存在、运行未成功或状态库失败时返回
    /// [`BacktestError`]。
    pub async fn compare(&self, ids: &[String]) -> Result<CompareResponse, BacktestError> {
        let ids = validate_compare_ids(ids, self.config.max_compare_runs)?;
        let store = Arc::clone(&self.store);
        let loaded = tokio::task::spawn_blocking(move || {
            let mut runs = Vec::with_capacity(ids.len());
            let mut curves = Vec::with_capacity(ids.len());
            for id in &ids {
                let run = store.get_backtest_run(id)?.ok_or(BacktestError::NotFound)?;
                curves.push(load_underwater(&store, id)?);
                runs.push(run);
            }
            Ok::<_, BacktestError>((runs, curves))
        })
        .await
        .map_err(|error| BacktestError::Worker(format!("state task join failed: {error}")))??;
        let (runs, curves) = loaded;
        for run in &runs {
            if run.status != super::model::RunStatus::Succeeded {
                return Err(BacktestError::Invalid(format!(
                    "run {} is not succeeded (status={})",
                    run.id,
                    run.status.as_str()
                )));
            }
        }
        Ok(build_compare(runs, curves, self.config.max_compare_runs))
    }

    /// 导出对比指标 CSV。
    ///
    /// # Errors
    /// 同 [`BacktestService::compare`]。
    pub async fn compare_csv(&self, ids: &[String]) -> Result<String, BacktestError> {
        let response = self.compare(ids).await?;
        Ok(export_csv(&response.runs))
    }

    async fn execute_batch(
        self: Arc<Self>,
        batch_id: String,
        plan: super::model::BacktestPlan,
        runs: Vec<(String, DualMaParams)>,
    ) {
        let permit = match Arc::clone(&self.semaphore).acquire_owned().await {
            Ok(permit) => permit,
            Err(error) => {
                tracing::error!(batch_id, error = %error, "backtest semaphore closed");
                self.fail_all(&runs, &format!("backtest semaphore closed: {error}"), None)
                    .await;
                return;
            }
        };

        for (id, _) in &runs {
            let store = Arc::clone(&self.store);
            let id_owned = id.clone();
            let started_at = now();
            if let Err(error) = tokio::task::spawn_blocking(move || {
                store.mark_backtest_running(&id_owned, started_at)
            })
            .await
            .unwrap_or_else(|join| {
                Err(crate::app::state::StateError::Corrupt(format!(
                    "state task join failed: {join}"
                )))
            }) {
                tracing::error!(run_id = %id, error = %error, "failed to mark backtest running");
            }
        }

        let batch_dir = self.output_dir.join(&batch_id);
        let request_path = batch_dir.join("request.json");
        let result_path = batch_dir.join("result.json");
        let log_path = batch_dir.join("worker.log");
        let request = WorkerRequest {
            data_dir: self.data_dir.to_string_lossy().into_owned(),
            market_type: plan.market_type.clone(),
            pair: plan.pair.clone(),
            timeframe: plan.timeframe.clone(),
            strategy: plan.strategy.clone(),
            params_list: plan.params_list.clone(),
            fee_bps: plan.fee_bps,
            slippage_bps: plan.slippage_bps,
            initial_balance: plan.initial_balance,
            start_ms: plan.start_ms,
            end_ms: plan.end_ms,
            max_curve_points: self.config.max_curve_points,
            max_trades: self.config.max_trades,
        };

        let invocation = self
            .invoke_worker(&batch_dir, &request, &request_path, &result_path, &log_path)
            .await;
        let log_text = invocation.log;
        if let Err(error) = tokio::fs::write(&log_path, &log_text).await {
            tracing::error!(path = %log_path.display(), error = %error, "failed to write worker log");
        }
        let log_path_string = Some(log_path.to_string_lossy().into_owned());

        match invocation.result {
            Ok(worker) => {
                let mut unmatched = runs.clone();
                for worker_run in worker.runs {
                    let Some(position) = unmatched.iter().position(|(_, params)| {
                        params.fast == worker_run.params.fast
                            && params.slow == worker_run.params.slow
                    }) else {
                        tracing::warn!(
                            batch_id,
                            params = %worker_run.params.text(),
                            "worker returned an unexpected params run"
                        );
                        continue;
                    };
                    let (id, _) = unmatched.remove(position);
                    if let Some(error) = worker_run.error {
                        self.mark_failed(
                            &id,
                            &format!("worker run failed: {error}"),
                            log_path_string.as_deref(),
                        )
                        .await;
                        continue;
                    }
                    let Some(metrics) = worker_run.metrics else {
                        self.mark_failed(
                            &id,
                            "worker returned no metrics",
                            log_path_string.as_deref(),
                        )
                        .await;
                        continue;
                    };
                    let result = StoredBacktestResult {
                        bars: worker_run.bars.max(worker.rows),
                        data_first_ms: worker_run.first_time,
                        data_last_ms: worker_run.last_time,
                        engine: worker.engine.clone(),
                        worker_version: worker.worker_version.clone(),
                        metrics,
                        trades: worker_run.trades,
                        trades_truncated: worker_run.trades_truncated,
                        drawdown_events: worker_run.drawdown_events,
                        equity: worker_run
                            .equity
                            .into_iter()
                            .map(|(time, value)| EquityPoint { time, value })
                            .collect(),
                        underwater: worker_run
                            .underwater
                            .into_iter()
                            .map(|(time, drawdown_pct)| UnderwaterPoint { time, drawdown_pct })
                            .collect(),
                        log_path: log_path_string.clone(),
                    };
                    let store = Arc::clone(&self.store);
                    let id_owned = id.clone();
                    let keep_equity = self.config.keep_equity;
                    let finished_at = now();
                    let outcome = tokio::task::spawn_blocking(move || {
                        store.complete_backtest(&id_owned, &result, keep_equity, finished_at)
                    })
                    .await;
                    match outcome {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => {
                            tracing::error!(run_id = %id, error = %error, "failed to persist backtest result");
                            self.mark_failed(
                                &id,
                                &format!("failed to persist result: {error}"),
                                log_path_string.as_deref(),
                            )
                            .await;
                        }
                        Err(join) => {
                            tracing::error!(run_id = %id, error = %join, "state task join failed");
                            self.mark_failed(
                                &id,
                                &format!("state task join failed: {join}"),
                                log_path_string.as_deref(),
                            )
                            .await;
                        }
                    }
                }
                for (id, params) in unmatched {
                    self.mark_failed(
                        &id,
                        &format!("worker returned no result for {}", params.text()),
                        log_path_string.as_deref(),
                    )
                    .await;
                }
            }
            Err(error) => {
                tracing::error!(batch_id, error = %error, "backtest worker failed");
                self.fail_all(&runs, &error, log_path_string.as_deref())
                    .await;
            }
        }
        drop(permit);
    }

    async fn invoke_worker(
        &self,
        batch_dir: &Path,
        request: &WorkerRequest,
        request_path: &Path,
        result_path: &Path,
        _log_path: &Path,
    ) -> WorkerInvocation {
        if let Err(error) = tokio::fs::create_dir_all(batch_dir).await {
            return WorkerInvocation {
                result: Err(format!("failed to create batch dir: {error}")),
                log: format!(
                    "failed to create batch dir {}: {error}\n",
                    batch_dir.display()
                ),
            };
        }
        let request_json = match serde_json::to_vec_pretty(request) {
            Ok(json) => json,
            Err(error) => {
                return WorkerInvocation {
                    result: Err(format!("failed to serialize request: {error}")),
                    log: format!("failed to serialize request: {error}\n"),
                };
            }
        };
        if let Err(error) = tokio::fs::write(request_path, &request_json).await {
            return WorkerInvocation {
                result: Err(format!("failed to write request: {error}")),
                log: format!(
                    "failed to write request {}: {error}\n",
                    request_path.display()
                ),
            };
        }

        let mut command = Command::new(&self.python_bin);
        command
            .current_dir(&self.worker_dir)
            .arg("-m")
            .arg("backtest_worker.cli")
            .arg("--request")
            .arg(request_path)
            .arg("--output")
            .arg(result_path)
            .env("PYTHONPATH", &self.worker_dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let mut log = String::new();
        log.push_str("command: ");
        log.push_str(&self.python_bin.to_string_lossy());
        log.push_str(" -m backtest_worker.cli --request ");
        log.push_str(&request_path.to_string_lossy());
        log.push_str(" --output ");
        log.push_str(&result_path.to_string_lossy());
        log.push('\n');
        log.push_str("request:\n");
        log.push_str(&String::from_utf8_lossy(&request_json));
        log.push('\n');

        let output = match tokio::time::timeout(
            Duration::from_secs(self.config.timeout_secs),
            command.output(),
        )
        .await
        {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => {
                log.push_str(&format!("spawn failed: {error}\n"));
                return WorkerInvocation {
                    result: Err(format!("failed to spawn python worker: {error}")),
                    log,
                };
            }
            Err(_) => {
                log.push_str(&format!(
                    "timeout after {} seconds\n",
                    self.config.timeout_secs
                ));
                return WorkerInvocation {
                    result: Err(format!(
                        "python worker timed out after {} seconds",
                        self.config.timeout_secs
                    )),
                    log,
                };
            }
        };

        log.push_str(&format!("exit: {}\n", output.status));
        log.push_str("stdout:\n");
        log.push_str(&String::from_utf8_lossy(&output.stdout));
        log.push_str("\nstderr:\n");
        log.push_str(&String::from_utf8_lossy(&output.stderr));
        log.push('\n');

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let tail = tail_chars(&stderr, 4000);
            return WorkerInvocation {
                result: Err(format!(
                    "python worker exited with {}: {}",
                    output
                        .status
                        .code()
                        .map_or_else(|| "signal".to_owned(), |code| code.to_string()),
                    tail.trim()
                )),
                log,
            };
        }

        match tokio::fs::read(result_path).await {
            Ok(bytes) => match serde_json::from_slice::<WorkerOutput>(&bytes) {
                Ok(worker) => WorkerInvocation {
                    result: Ok(worker),
                    log,
                },
                Err(error) => {
                    log.push_str(&format!("invalid result JSON: {error}\n"));
                    WorkerInvocation {
                        result: Err(format!("invalid worker result JSON: {error}")),
                        log,
                    }
                }
            },
            Err(error) => WorkerInvocation {
                result: Err(format!(
                    "failed to read worker result {}: {error}",
                    result_path.display()
                )),
                log,
            },
        }
    }

    async fn fail_all(&self, runs: &[(String, DualMaParams)], error: &str, log_path: Option<&str>) {
        for (id, _) in runs {
            self.mark_failed(id, error, log_path).await;
        }
    }

    async fn mark_failed(&self, id: &str, error: &str, log_path: Option<&str>) {
        let store = Arc::clone(&self.store);
        let id_owned = id.to_owned();
        let error = tail_chars(error, 8000);
        let log_owned = log_path.map(str::to_owned);
        let finished_at = now();
        let outcome = tokio::task::spawn_blocking(move || {
            store.mark_backtest_failed(&id_owned, &error, log_owned.as_deref(), finished_at)
        })
        .await;
        if let Ok(Err(error)) = outcome {
            tracing::error!(run_id = %id, error = %error, "failed to mark backtest failed");
        }
    }
}

fn validate_compare_ids(ids: &[String], max: usize) -> Result<Vec<String>, BacktestError> {
    if ids.len() < 2 {
        return Err(BacktestError::Invalid(
            "compare requires at least 2 runs".into(),
        ));
    }
    if ids.len() > max {
        return Err(BacktestError::Invalid(format!(
            "compare allows at most {max} runs, got {}",
            ids.len()
        )));
    }
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if id.trim().is_empty() {
            return Err(BacktestError::Invalid("empty run id".into()));
        }
        if !seen.insert(id) {
            return Err(BacktestError::Invalid(format!("duplicate run id: {id}")));
        }
        out.push(id.clone());
    }
    Ok(out)
}

fn resolve_python_bin(path: &Path) -> PathBuf {
    let components = path.components().count();
    if path.is_absolute() || components <= 1 {
        return path.to_path_buf();
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(path)
}

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn tail_chars(text: &str, max: usize) -> String {
    let chars = text.chars().count();
    if chars <= max {
        return text.to_owned();
    }
    text.chars().skip(chars - max).collect()
}

#[derive(Debug, Serialize)]
struct WorkerRequest {
    data_dir: String,
    market_type: String,
    pair: String,
    timeframe: String,
    strategy: String,
    params_list: Vec<DualMaParams>,
    fee_bps: f64,
    slippage_bps: f64,
    initial_balance: f64,
    start_ms: Option<i64>,
    end_ms: Option<i64>,
    max_curve_points: usize,
    max_trades: usize,
}

#[derive(Debug, Deserialize)]
struct WorkerOutput {
    worker_version: String,
    engine: String,
    rows: i64,
    #[allow(dead_code)]
    first_time: i64,
    #[allow(dead_code)]
    last_time: i64,
    runs: Vec<WorkerRun>,
}

#[derive(Debug, Deserialize)]
struct WorkerRun {
    params: DualMaParams,
    #[allow(dead_code)]
    params_text: String,
    bars: i64,
    first_time: i64,
    last_time: i64,
    metrics: Option<BacktestMetrics>,
    trades: Vec<BacktestTrade>,
    trades_truncated: bool,
    drawdown_events: Vec<DrawdownEvent>,
    equity: Vec<(i64, f64)>,
    underwater: Vec<(i64, f64)>,
    error: Option<String>,
}

struct WorkerInvocation {
    result: Result<WorkerOutput, String>,
    log: String,
}
