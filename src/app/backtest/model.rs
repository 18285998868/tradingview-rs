//! 回测请求、运行记录、指标与对比的数据模型。
//!
//! 这里的字段名同时是 HTTP JSON 与 Python worker 结果的契约。改字段必须同步
//! `python/backtest_worker/` 与 `tests/backtest_test.rs`。

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::app::state::StateError;
use crate::config::BacktestConfig;
use crate::utils::{valid_pair, valid_timeframe};

/// 唯一支持的策略标识。
pub const STRATEGY_DUAL_MA: &str = "dual_ma";
/// 策略实现版本；算法口径变化时递增。
pub const STRATEGY_VERSION: &str = "1";
/// 首版支持的 12 个原生周期。
pub const BACKTEST_TIMEFRAMES: [&str; 12] = [
    "1m", "3m", "5m", "15m", "30m", "1h", "2h", "4h", "6h", "8h", "12h", "1d",
];

/// 双均线参数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DualMaParams {
    /// 快线周期。
    pub fast: u32,
    /// 慢线周期，必须大于快线。
    pub slow: u32,
}

impl Default for DualMaParams {
    fn default() -> Self {
        Self { fast: 10, slow: 30 }
    }
}

impl DualMaParams {
    /// 规整的参数文本（用于列表展示与扫描映射）。
    pub fn text(&self) -> String {
        format!("fast={},slow={}", self.fast, self.slow)
    }

    /// 校验参数语义。
    ///
    /// # Errors
    /// `fast < 1`、`slow < 2` 或 `fast >= slow` 时返回错误消息。
    pub fn validate(&self) -> Result<(), String> {
        if self.fast < 1 {
            return Err("fast must be >= 1".into());
        }
        if self.slow < 2 {
            return Err("slow must be >= 2".into());
        }
        if self.fast >= self.slow {
            return Err("fast must be < slow".into());
        }
        Ok(())
    }
}

/// 参数扫描：缺省轴使用基础参数。两轴都给时做笛卡尔积。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParamGrid {
    /// 快线候选值。
    pub fast: Option<Vec<u32>>,
    /// 慢线候选值。
    pub slow: Option<Vec<u32>>,
}

/// 创建回测请求。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateBacktestRequest {
    /// 市场类型；首版只允许 `spot`，缺省为 `spot`。
    pub market_type: Option<String>,
    /// 交易对，例如 `BTCUSDT`。
    pub pair: String,
    /// 周期。
    pub timeframe: String,
    /// 策略；缺省 `dual_ma`。
    pub strategy: Option<String>,
    /// 方向；首版只允许 `long`（缺省）。
    pub direction: Option<String>,
    /// 单组参数；缺省 `fast=10, slow=30`。
    pub params: Option<DualMaParams>,
    /// 可选参数扫描。
    pub param_grid: Option<ParamGrid>,
    /// 手续费，单位 bps；缺省 10。
    #[serde(default = "default_fee_bps")]
    pub fee_bps: f64,
    /// 滑点，单位 bps；缺省 5。
    #[serde(default = "default_slippage_bps")]
    pub slippage_bps: f64,
    /// 初始资金；缺省 10000。
    pub initial_balance: Option<f64>,
    /// 数据起始时间（含，毫秒）。
    pub start_ms: Option<i64>,
    /// 数据结束时间（不含，毫秒）。
    pub end_ms: Option<i64>,
}

fn default_fee_bps() -> f64 {
    10.0
}

fn default_slippage_bps() -> f64 {
    5.0
}

/// 校验并展开后的执行计划。
#[derive(Debug, Clone, PartialEq)]
pub struct BacktestPlan {
    /// 固定 `spot`。
    pub market_type: String,
    /// 交易对。
    pub pair: String,
    /// 周期。
    pub timeframe: String,
    /// 策略。
    pub strategy: String,
    /// 策略版本。
    pub strategy_version: String,
    /// 展开后的参数组。
    pub params_list: Vec<DualMaParams>,
    /// 手续费 bps。
    pub fee_bps: f64,
    /// 滑点 bps。
    pub slippage_bps: f64,
    /// 初始资金。
    pub initial_balance: f64,
    /// 起始时间。
    pub start_ms: Option<i64>,
    /// 结束时间。
    pub end_ms: Option<i64>,
}

impl CreateBacktestRequest {
    /// 校验请求并按配置展开参数网格。
    ///
    /// # Errors
    /// 市场/方向/周期/参数/成本越界或扫描数量超限时返回
    /// [`BacktestError::Invalid`]。
    pub fn plan(&self, config: &BacktestConfig) -> Result<BacktestPlan, BacktestError> {
        let market = match self.market_type.as_deref() {
            None | Some("spot") | Some("SPOT") => "spot".to_owned(),
            Some(other) => {
                return Err(BacktestError::Invalid(format!(
                    "only Binance spot is supported, got market_type={other}"
                )));
            }
        };
        if !valid_pair(&self.pair) {
            return Err(BacktestError::Invalid("invalid pair".into()));
        }
        if !valid_timeframe(&self.timeframe) {
            return Err(BacktestError::Invalid("invalid timeframe".into()));
        }
        if !BACKTEST_TIMEFRAMES.contains(&self.timeframe.as_str()) {
            return Err(BacktestError::Invalid(format!(
                "timeframe {} is not one of {}",
                self.timeframe,
                BACKTEST_TIMEFRAMES.join(",")
            )));
        }
        let strategy = self.strategy.as_deref().unwrap_or(STRATEGY_DUAL_MA);
        if strategy != STRATEGY_DUAL_MA {
            return Err(BacktestError::Invalid(format!(
                "unsupported strategy {strategy}, only {STRATEGY_DUAL_MA} is available"
            )));
        }
        let direction = self.direction.as_deref().unwrap_or("long");
        if direction != "long" {
            return Err(BacktestError::Invalid(
                "spot short/leverage is not supported in P0; direction must be long".into(),
            ));
        }

        if !self.fee_bps.is_finite() || !(0.0..=10_000.0).contains(&self.fee_bps) {
            return Err(BacktestError::Invalid(
                "fee_bps must be finite and in 0..=10000".into(),
            ));
        }
        if !self.slippage_bps.is_finite() || !(0.0..=10_000.0).contains(&self.slippage_bps) {
            return Err(BacktestError::Invalid(
                "slippage_bps must be finite and in 0..=10000".into(),
            ));
        }
        let initial_balance = self.initial_balance.unwrap_or(10_000.0);
        if !initial_balance.is_finite() || initial_balance <= 0.0 {
            return Err(BacktestError::Invalid(
                "initial_balance must be a finite number > 0".into(),
            ));
        }
        if let (Some(start), Some(end)) = (self.start_ms, self.end_ms)
            && start >= end
        {
            return Err(BacktestError::Invalid("start_ms must be < end_ms".into()));
        }
        if self.start_ms.is_some_and(|value| value < 0) {
            return Err(BacktestError::Invalid("start_ms must be >= 0".into()));
        }
        if self.end_ms.is_some_and(|value| value < 0) {
            return Err(BacktestError::Invalid("end_ms must be >= 0".into()));
        }

        let base = self.params.unwrap_or_default();
        base.validate().map_err(BacktestError::Invalid)?;
        let mut params_list = Vec::new();
        let mut seen = HashSet::new();
        match &self.param_grid {
            None => params_list.push(base),
            Some(grid) => {
                if grid.fast.as_ref().is_some_and(Vec::is_empty)
                    || grid.slow.as_ref().is_some_and(Vec::is_empty)
                {
                    return Err(BacktestError::Invalid(
                        "param_grid axes must not be empty".into(),
                    ));
                }
                let default_fasts = [base.fast];
                let default_slows = [base.slow];
                let fasts = grid.fast.as_deref().unwrap_or(&default_fasts);
                let slows = grid.slow.as_deref().unwrap_or(&default_slows);
                for &fast in fasts {
                    for &slow in slows {
                        let params = DualMaParams { fast, slow };
                        params.validate().map_err(|reason| {
                            BacktestError::Invalid(format!(
                                "invalid grid params fast={fast},slow={slow}: {reason}"
                            ))
                        })?;
                        if seen.insert((fast, slow)) {
                            params_list.push(params);
                        }
                    }
                }
            }
        }
        if params_list.len() > config.max_grid_runs {
            return Err(BacktestError::Invalid(format!(
                "param grid expands to {} runs, max is {}",
                params_list.len(),
                config.max_grid_runs
            )));
        }

        Ok(BacktestPlan {
            market_type: market,
            pair: self.pair.clone(),
            timeframe: self.timeframe.clone(),
            strategy: strategy.to_owned(),
            strategy_version: STRATEGY_VERSION.to_owned(),
            params_list,
            fee_bps: self.fee_bps,
            slippage_bps: self.slippage_bps,
            initial_balance,
            start_ms: self.start_ms,
            end_ms: self.end_ms,
        })
    }
}

/// 运行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    /// 已入队，等待 worker 许可。
    Queued,
    /// worker 执行中。
    Running,
    /// 成功并已持久化结果。
    Succeeded,
    /// 失败，可查看日志并重试。
    Failed,
}

impl RunStatus {
    /// 数据库字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }

    /// 解析数据库字符串。
    ///
    /// # Errors
    /// 非法状态返回 [`StateError::Corrupt`]。
    pub fn parse(value: &str) -> Result<Self, StateError> {
        match value {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            other => Err(StateError::Corrupt(format!(
                "unknown backtest status {other}"
            ))),
        }
    }
}

/// 汇总指标。`None` 表示数学上无定义（例如零波动下的夏普）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BacktestMetrics {
    /// 交易笔数。
    pub num_trades: i64,
    /// 胜率，百分比。
    pub win_rate: Option<f64>,
    /// 总收益，百分比。
    pub total_return_pct: Option<f64>,
    /// 夏普（年化，无风险利率 0）。
    pub sharpe_ratio: Option<f64>,
    /// 索提诺（年化，无风险利率 0）。
    pub sortino_ratio: Option<f64>,
    /// 最大回撤，百分比正数。
    pub max_drawdown_pct: Option<f64>,
    /// 平均回撤，百分比正数。
    pub avg_drawdown_pct: Option<f64>,
    /// 回撤事件数。
    pub drawdown_count: i64,
    /// 最长回撤持续期（天）。
    pub longest_drawdown_days: Option<f64>,
    /// 最长恢复期（天）。
    pub longest_recovery_days: Option<f64>,
    /// 处于水下状态的时间占比，百分比。
    pub drawdown_time_pct: Option<f64>,
    /// 盈亏比。
    pub profit_factor: Option<f64>,
    /// 单笔期望收益（小数）。
    pub expectancy: Option<f64>,
    /// 期末权益。
    pub final_equity: Option<f64>,
    /// bar 数。
    pub bars: i64,
    /// 年化周期数。
    pub periods_per_year: f64,
}

/// 运行摘要（列表与详情共用）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BacktestRunSummary {
    /// 运行 ID。
    pub id: String,
    /// 同一次参数扫描的批次 ID。
    pub batch_id: String,
    /// 状态。
    pub status: RunStatus,
    /// 市场类型。
    pub market_type: String,
    /// 交易对。
    pub pair: String,
    /// 周期。
    pub timeframe: String,
    /// 策略。
    pub strategy: String,
    /// 策略版本。
    pub strategy_version: String,
    /// 参数。
    pub params: DualMaParams,
    /// 参数文本。
    pub params_text: String,
    /// 手续费 bps。
    pub fee_bps: f64,
    /// 滑点 bps。
    pub slippage_bps: f64,
    /// 初始资金。
    pub initial_balance: f64,
    /// 起始时间。
    pub start_ms: Option<i64>,
    /// 结束时间。
    pub end_ms: Option<i64>,
    /// 创建时间。
    pub created_at: i64,
    /// 开始时间。
    pub started_at: Option<i64>,
    /// 完成时间。
    pub finished_at: Option<i64>,
    /// 失败原因。
    pub error: Option<String>,
    /// 日志路径。
    pub log_path: Option<String>,
    /// bar 数。
    pub bars: Option<i64>,
    /// 实际数据首 bar。
    pub data_first_ms: Option<i64>,
    /// 实际数据末 bar。
    pub data_last_ms: Option<i64>,
    /// 引擎版本。
    pub engine: Option<String>,
    /// worker 版本。
    pub worker_version: Option<String>,
    /// 指标。
    pub metrics: Option<BacktestMetrics>,
    /// 交易明细是否被截断。
    pub trades_truncated: bool,
    /// 保留策略。
    pub retention: String,
}

/// 交易明细。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BacktestTrade {
    /// 序号。
    pub seq: i64,
    /// 入场时间。
    pub entry_time: i64,
    /// 出场时间。
    pub exit_time: i64,
    /// 方向。
    pub side: String,
    /// `open` / `closed`。
    pub status: String,
    /// 入场价。
    pub entry_price: Option<f64>,
    /// 出场价。
    pub exit_price: Option<f64>,
    /// 数量。
    pub qty: Option<f64>,
    /// 盈亏。
    pub pnl: Option<f64>,
    /// 盈亏百分比。
    pub pnl_pct: Option<f64>,
    /// 持有 bar 数。
    pub bars_held: i64,
}

/// 回撤事件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrawdownEvent {
    /// 序号。
    pub seq: i64,
    /// 峰值时间。
    pub peak_time: i64,
    /// 谷底时间。
    pub trough_time: i64,
    /// 恢复时间；未恢复为 `null`。
    pub recovery_time: Option<i64>,
    /// 深度，百分比正数。
    pub depth_pct: f64,
    /// 峰值到恢复/最后的持续期。
    pub duration_ms: i64,
    /// 谷底到恢复的持续期。
    pub recovery_ms: Option<i64>,
    /// 是否恢复。
    pub recovered: bool,
}

/// 权益点。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EquityPoint {
    /// 时间（毫秒）。
    pub time: i64,
    /// 权益。
    pub value: f64,
}

/// 水下曲线点。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct UnderwaterPoint {
    /// 时间（毫秒）。
    pub time: i64,
    /// 回撤百分比，`<= 0`。
    pub drawdown_pct: f64,
}

/// 图表买卖点。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrategyMarker {
    /// 时间（毫秒）。
    pub time: i64,
    /// `long`。
    pub side: String,
    /// 成交价。
    pub price: Option<f64>,
    /// `entry` / `exit`。
    pub kind: String,
}

/// 运行详情。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BacktestRunDetail {
    /// 运行摘要。
    pub run: BacktestRunSummary,
    /// 交易明细。
    pub trades: Vec<BacktestTrade>,
    /// 回撤事件。
    pub drawdown_events: Vec<DrawdownEvent>,
    /// 降采样权益。
    pub equity: Vec<EquityPoint>,
    /// 降采样水下曲线。
    pub underwater: Vec<UnderwaterPoint>,
    /// 由交易派生的图表标记。
    pub markers: Vec<StrategyMarker>,
}

/// 创建成功后的响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitOutcome {
    /// 批次 ID。
    pub batch_id: String,
    /// 入队运行。
    pub runs: Vec<BacktestRunSummary>,
}

/// 对比请求。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompareRequest {
    /// 运行 ID 列表，2..=配置上限。
    pub ids: Vec<String>,
}

/// 命名对比名称最大字符数。
pub const MAX_COMPARISON_NAME_CHARS: usize = 64;

/// 命名保存的对比摘要。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BacktestComparison {
    /// 对比 ID。
    pub id: String,
    /// 名称，唯一。
    pub name: String,
    /// 运行 ID 列表，按保存顺序。
    pub run_ids: Vec<String>,
    /// 读取时已不存在的运行数；大于 0 时找回会缺曲线。
    pub missing_runs: usize,
    /// 创建时间（毫秒）。
    pub created_at: i64,
    /// 最后更新时间（毫秒）。
    pub updated_at: i64,
}

/// 保存命名对比的请求。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveComparisonRequest {
    /// 名称，1..=64 字符。
    pub name: String,
    /// 运行 ID 列表，2..=配置上限，必须存在且已成功。
    pub ids: Vec<String>,
}

/// 对比警告。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompareWarning {
    /// 警告类别。
    pub kind: String,
    /// 人类可读说明。
    pub message: String,
    /// 涉及的运行 ID。
    pub run_ids: Vec<String>,
}

/// 对齐后的可选点。`value = null` 表示该运行在此时间无数据。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlignedPoint {
    /// 时间。
    pub time: i64,
    /// 回撤百分比；`null` 显式表示缺失。
    pub drawdown_pct: Option<f64>,
}

/// 单个运行的对齐曲线。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompareSeries {
    /// 运行 ID。
    pub run_id: String,
    /// 展示标签。
    pub label: String,
    /// 参数文本。
    pub params_text: String,
    /// 对齐后的点。
    pub points: Vec<AlignedPoint>,
    /// 缺失点数。
    pub missing_points: usize,
}

/// 对比响应。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompareResponse {
    /// 运行摘要。
    pub runs: Vec<BacktestRunSummary>,
    /// 元数据警告。
    pub warnings: Vec<CompareWarning>,
    /// 按时间并集对齐的曲线。
    pub series: Vec<CompareSeries>,
    /// 并集时间点数。
    pub union_points: usize,
    /// 最大对比数量。
    pub max_compare_runs: usize,
}

/// 回测子系统错误。
#[derive(Debug, Error)]
pub enum BacktestError {
    /// 请求不合法。
    #[error("invalid backtest request: {0}")]
    Invalid(String),
    /// 运行不存在。
    #[error("backtest run not found")]
    NotFound,
    /// 功能被配置关闭。
    #[error("backtest is disabled")]
    Disabled,
    /// worker 执行或结果解析失败。
    #[error("backtest worker failed: {0}")]
    Worker(String),
    /// 状态库失败。
    #[error(transparent)]
    State(#[from] StateError),
}
