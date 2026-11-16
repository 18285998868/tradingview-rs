//! `StateStore` 的回测持久化扩展。
//!
//! 表结构见 `migrations/002_backtest.sql`。所有写操作在事务内完成；曲线与交易
//! 先删后插，保证重跑幂等。

use rusqlite::{OptionalExtension, Row, params};

use crate::app::state::{StateResult, StateStore};

use super::model::{
    BacktestMetrics, BacktestPlan, BacktestRunDetail, BacktestRunSummary, BacktestTrade,
    DrawdownEvent, DualMaParams, EquityPoint, RunStatus, StrategyMarker, UnderwaterPoint,
};

const RUN_COLUMNS: &str = "id, batch_id, status, market_type, pair, timeframe, strategy, \
     strategy_version, params_json, params_text, fee_bps, slippage_bps, initial_balance, \
     start_ms, end_ms, created_at, started_at, finished_at, error, log_path, bars, \
     data_first_ms, data_last_ms, engine, worker_version, metrics_json, trades_truncated, retention";

struct StoredRun {
    id: String,
    batch_id: String,
    status: String,
    market_type: String,
    pair: String,
    timeframe: String,
    strategy: String,
    strategy_version: String,
    params_json: String,
    params_text: String,
    fee_bps: f64,
    slippage_bps: f64,
    initial_balance: f64,
    start_ms: Option<i64>,
    end_ms: Option<i64>,
    created_at: i64,
    started_at: Option<i64>,
    finished_at: Option<i64>,
    error: Option<String>,
    log_path: Option<String>,
    bars: Option<i64>,
    data_first_ms: Option<i64>,
    data_last_ms: Option<i64>,
    engine: Option<String>,
    worker_version: Option<String>,
    metrics_json: Option<String>,
    trades_truncated: bool,
    retention: String,
}

impl StoredRun {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            batch_id: row.get(1)?,
            status: row.get(2)?,
            market_type: row.get(3)?,
            pair: row.get(4)?,
            timeframe: row.get(5)?,
            strategy: row.get(6)?,
            strategy_version: row.get(7)?,
            params_json: row.get(8)?,
            params_text: row.get(9)?,
            fee_bps: row.get(10)?,
            slippage_bps: row.get(11)?,
            initial_balance: row.get(12)?,
            start_ms: row.get(13)?,
            end_ms: row.get(14)?,
            created_at: row.get(15)?,
            started_at: row.get(16)?,
            finished_at: row.get(17)?,
            error: row.get(18)?,
            log_path: row.get(19)?,
            bars: row.get(20)?,
            data_first_ms: row.get(21)?,
            data_last_ms: row.get(22)?,
            engine: row.get(23)?,
            worker_version: row.get(24)?,
            metrics_json: row.get(25)?,
            trades_truncated: row.get::<_, i64>(26)? != 0,
            retention: row.get(27)?,
        })
    }

    fn into_summary(self) -> StateResult<BacktestRunSummary> {
        let params: DualMaParams = serde_json::from_str(&self.params_json)?;
        let metrics: Option<BacktestMetrics> = match self.metrics_json {
            Some(json) => Some(serde_json::from_str(&json)?),
            None => None,
        };
        Ok(BacktestRunSummary {
            id: self.id,
            batch_id: self.batch_id,
            status: RunStatus::parse(&self.status)?,
            market_type: self.market_type,
            pair: self.pair,
            timeframe: self.timeframe,
            strategy: self.strategy,
            strategy_version: self.strategy_version,
            params,
            params_text: self.params_text,
            fee_bps: self.fee_bps,
            slippage_bps: self.slippage_bps,
            initial_balance: self.initial_balance,
            start_ms: self.start_ms,
            end_ms: self.end_ms,
            created_at: self.created_at,
            started_at: self.started_at,
            finished_at: self.finished_at,
            error: self.error,
            log_path: self.log_path,
            bars: self.bars,
            data_first_ms: self.data_first_ms,
            data_last_ms: self.data_last_ms,
            engine: self.engine,
            worker_version: self.worker_version,
            metrics,
            trades_truncated: self.trades_truncated,
            retention: self.retention,
        })
    }
}

/// 写库用的完整结果。
pub(crate) struct StoredBacktestResult {
    pub bars: i64,
    pub data_first_ms: i64,
    pub data_last_ms: i64,
    pub engine: String,
    pub worker_version: String,
    pub metrics: BacktestMetrics,
    pub trades: Vec<BacktestTrade>,
    pub trades_truncated: bool,
    pub drawdown_events: Vec<DrawdownEvent>,
    pub equity: Vec<EquityPoint>,
    pub underwater: Vec<UnderwaterPoint>,
    pub log_path: Option<String>,
}

impl StateStore {
    /// 批量插入排队中的运行。
    ///
    /// # Errors
    /// SQLite 或 JSON 序列化失败时返回 [`crate::app::state::StateError`]。
    pub(crate) fn create_backtest_runs(
        &self,
        batch_id: &str,
        plan: &BacktestPlan,
        runs: &[(String, DualMaParams)],
        keep_equity: bool,
        created_at: i64,
    ) -> StateResult<Vec<BacktestRunSummary>> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        let retention = if keep_equity {
            "downsampled_equity"
        } else {
            "underwater_only"
        };
        let mut out = Vec::with_capacity(runs.len());
        for (id, params) in runs {
            let params_json = serde_json::to_string(params)?;
            let params_text = params.text();
            tx.execute(
                "INSERT INTO backtest_runs
                    (id, batch_id, status, market_type, pair, timeframe, strategy,
                     strategy_version, params_json, params_text, fee_bps, slippage_bps,
                     initial_balance, start_ms, end_ms, created_at, retention)
                 VALUES (?1, ?2, 'queued', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
                params![
                    id,
                    batch_id,
                    plan.market_type,
                    plan.pair,
                    plan.timeframe,
                    plan.strategy,
                    plan.strategy_version,
                    params_json,
                    params_text,
                    plan.fee_bps,
                    plan.slippage_bps,
                    plan.initial_balance,
                    plan.start_ms,
                    plan.end_ms,
                    created_at,
                    retention,
                ],
            )?;
            out.push(BacktestRunSummary {
                id: id.clone(),
                batch_id: batch_id.to_owned(),
                status: RunStatus::Queued,
                market_type: plan.market_type.clone(),
                pair: plan.pair.clone(),
                timeframe: plan.timeframe.clone(),
                strategy: plan.strategy.clone(),
                strategy_version: plan.strategy_version.clone(),
                params: *params,
                params_text,
                fee_bps: plan.fee_bps,
                slippage_bps: plan.slippage_bps,
                initial_balance: plan.initial_balance,
                start_ms: plan.start_ms,
                end_ms: plan.end_ms,
                created_at,
                started_at: None,
                finished_at: None,
                error: None,
                log_path: None,
                bars: None,
                data_first_ms: None,
                data_last_ms: None,
                engine: None,
                worker_version: None,
                metrics: None,
                trades_truncated: false,
                retention: retention.to_owned(),
            });
        }
        tx.commit()?;
        Ok(out)
    }

    /// 读取单个运行。
    ///
    /// # Errors
    /// SQLite 或已存 JSON 损坏时返回 [`crate::app::state::StateError`]。
    pub(crate) fn get_backtest_run(&self, id: &str) -> StateResult<Option<BacktestRunSummary>> {
        let conn = self.conn()?;
        let stored = conn
            .query_row(
                &format!("SELECT {RUN_COLUMNS} FROM backtest_runs WHERE id = ?1"),
                params![id],
                StoredRun::from_row,
            )
            .optional()?;
        stored.map(StoredRun::into_summary).transpose()
    }

    /// 列出最近的运行。
    ///
    /// # Errors
    /// SQLite 或已存 JSON 损坏时返回 [`crate::app::state::StateError`]。
    pub(crate) fn list_backtest_runs(&self, limit: usize) -> StateResult<Vec<BacktestRunSummary>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {RUN_COLUMNS} FROM backtest_runs ORDER BY created_at DESC, id DESC LIMIT ?1"
        ))?;
        let rows = stmt.query_map(
            params![i64::try_from(limit).unwrap_or(i64::MAX)],
            StoredRun::from_row,
        )?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?.into_summary()?);
        }
        Ok(out)
    }

    /// 标记运行开始。
    ///
    /// # Errors
    /// SQLite 失败时返回 [`crate::app::state::StateError`]。
    pub(crate) fn mark_backtest_running(&self, id: &str, started_at: i64) -> StateResult<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE backtest_runs
             SET status = 'running', started_at = ?2, finished_at = NULL, error = NULL
             WHERE id = ?1",
            params![id, started_at],
        )?;
        Ok(())
    }

    /// 标记运行失败。
    ///
    /// # Errors
    /// SQLite 失败时返回 [`crate::app::state::StateError`]。
    pub(crate) fn mark_backtest_failed(
        &self,
        id: &str,
        error: &str,
        log_path: Option<&str>,
        finished_at: i64,
    ) -> StateResult<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE backtest_runs
             SET status = 'failed', finished_at = ?2, error = ?3, log_path = COALESCE(?4, log_path)
             WHERE id = ?1",
            params![id, finished_at, error, log_path],
        )?;
        Ok(())
    }

    /// 成功写入完整结果。
    ///
    /// # Errors
    /// SQLite 或 JSON 序列化失败时返回 [`crate::app::state::StateError`]。
    pub(crate) fn complete_backtest(
        &self,
        id: &str,
        result: &StoredBacktestResult,
        keep_equity: bool,
        finished_at: i64,
    ) -> StateResult<()> {
        let metrics_json = serde_json::to_string(&result.metrics)?;
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM backtest_trades WHERE run_id = ?1", params![id])?;
        tx.execute(
            "DELETE FROM backtest_drawdown_events WHERE run_id = ?1",
            params![id],
        )?;
        tx.execute("DELETE FROM backtest_equity WHERE run_id = ?1", params![id])?;
        tx.execute(
            "DELETE FROM backtest_underwater WHERE run_id = ?1",
            params![id],
        )?;

        for (index, trade) in result.trades.iter().enumerate() {
            tx.execute(
                "INSERT INTO backtest_trades
                    (run_id, seq, entry_time, exit_time, side, status, entry_price,
                     exit_price, qty, pnl, pnl_pct, bars_held)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    id,
                    i64::try_from(index).unwrap_or(i64::MAX),
                    trade.entry_time,
                    trade.exit_time,
                    trade.side,
                    trade.status,
                    trade.entry_price,
                    trade.exit_price,
                    trade.qty,
                    trade.pnl,
                    trade.pnl_pct,
                    trade.bars_held,
                ],
            )?;
        }
        for (index, event) in result.drawdown_events.iter().enumerate() {
            tx.execute(
                "INSERT INTO backtest_drawdown_events
                    (run_id, seq, peak_time, trough_time, recovery_time, depth_pct,
                     duration_ms, recovery_ms, recovered)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    id,
                    i64::try_from(index).unwrap_or(i64::MAX),
                    event.peak_time,
                    event.trough_time,
                    event.recovery_time,
                    event.depth_pct,
                    event.duration_ms,
                    event.recovery_ms,
                    if event.recovered { 1 } else { 0 },
                ],
            )?;
        }
        for (index, point) in result.underwater.iter().enumerate() {
            tx.execute(
                "INSERT INTO backtest_underwater (run_id, seq, time_ms, drawdown_pct)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    id,
                    i64::try_from(index).unwrap_or(i64::MAX),
                    point.time,
                    point.drawdown_pct
                ],
            )?;
        }
        if keep_equity {
            for (index, point) in result.equity.iter().enumerate() {
                tx.execute(
                    "INSERT INTO backtest_equity (run_id, seq, time_ms, value)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        id,
                        i64::try_from(index).unwrap_or(i64::MAX),
                        point.time,
                        point.value
                    ],
                )?;
            }
        }

        tx.execute(
            "UPDATE backtest_runs
             SET status = 'succeeded', finished_at = ?2, error = NULL, log_path = ?3,
                 bars = ?4, data_first_ms = ?5, data_last_ms = ?6, engine = ?7,
                 worker_version = ?8, metrics_json = ?9, trades_truncated = ?10
             WHERE id = ?1",
            params![
                id,
                finished_at,
                result.log_path,
                result.bars,
                result.data_first_ms,
                result.data_last_ms,
                result.engine,
                result.worker_version,
                metrics_json,
                if result.trades_truncated { 1 } else { 0 },
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 服务启动时把中断的运行标记为失败。
    ///
    /// # Errors
    /// SQLite 失败时返回 [`crate::app::state::StateError`]。
    pub(crate) fn mark_interrupted_backtests(&self, now: i64) -> StateResult<usize> {
        let conn = self.conn()?;
        let changed = conn.execute(
            "UPDATE backtest_runs
             SET status = 'failed', finished_at = ?1,
                 error = 'server restarted before the worker finished'
             WHERE status IN ('queued', 'running')",
            params![now],
        )?;
        Ok(changed)
    }

    /// 删除运行；外键级联删除明细。
    ///
    /// # Errors
    /// SQLite 失败时返回 [`crate::app::state::StateError`]。
    pub(crate) fn delete_backtest(&self, id: &str) -> StateResult<bool> {
        let conn = self.conn()?;
        let changed = conn.execute("DELETE FROM backtest_runs WHERE id = ?1", params![id])?;
        Ok(changed > 0)
    }

    /// 读取运行详情。
    ///
    /// # Errors
    /// SQLite 或 JSON 损坏时返回 [`crate::app::state::StateError`]。
    pub(crate) fn get_backtest_detail(&self, id: &str) -> StateResult<Option<BacktestRunDetail>> {
        let Some(run) = self.get_backtest_run(id)? else {
            return Ok(None);
        };
        let conn = self.conn()?;

        let mut trade_stmt = conn.prepare(
            "SELECT entry_time, exit_time, side, status, entry_price, exit_price, qty,
                    pnl, pnl_pct, bars_held
             FROM backtest_trades WHERE run_id = ?1 ORDER BY seq",
        )?;
        let trades = trade_stmt
            .query_map(params![id], |row| {
                Ok(BacktestTrade {
                    seq: 0,
                    entry_time: row.get(0)?,
                    exit_time: row.get(1)?,
                    side: row.get(2)?,
                    status: row.get(3)?,
                    entry_price: row.get(4)?,
                    exit_price: row.get(5)?,
                    qty: row.get(6)?,
                    pnl: row.get(7)?,
                    pnl_pct: row.get(8)?,
                    bars_held: row.get(9)?,
                })
            })?
            .enumerate()
            .map(|(index, row)| {
                let mut trade = row?;
                trade.seq = i64::try_from(index).unwrap_or(i64::MAX);
                Ok(trade)
            })
            .collect::<Result<Vec<_>, rusqlite::Error>>()?;

        let mut event_stmt = conn.prepare(
            "SELECT peak_time, trough_time, recovery_time, depth_pct, duration_ms,
                    recovery_ms, recovered
             FROM backtest_drawdown_events WHERE run_id = ?1 ORDER BY seq",
        )?;
        let drawdown_events = event_stmt
            .query_map(params![id], |row| {
                Ok(DrawdownEvent {
                    seq: 0,
                    peak_time: row.get(0)?,
                    trough_time: row.get(1)?,
                    recovery_time: row.get(2)?,
                    depth_pct: row.get(3)?,
                    duration_ms: row.get(4)?,
                    recovery_ms: row.get(5)?,
                    recovered: row.get::<_, i64>(6)? != 0,
                })
            })?
            .enumerate()
            .map(|(index, row)| {
                let mut event = row?;
                event.seq = i64::try_from(index).unwrap_or(i64::MAX);
                Ok(event)
            })
            .collect::<Result<Vec<_>, rusqlite::Error>>()?;

        let mut equity_stmt = conn
            .prepare("SELECT time_ms, value FROM backtest_equity WHERE run_id = ?1 ORDER BY seq")?;
        let equity = equity_stmt
            .query_map(params![id], |row| {
                Ok(EquityPoint {
                    time: row.get(0)?,
                    value: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, rusqlite::Error>>()?;

        let mut underwater_stmt = conn.prepare(
            "SELECT time_ms, drawdown_pct FROM backtest_underwater WHERE run_id = ?1 ORDER BY seq",
        )?;
        let underwater = underwater_stmt
            .query_map(params![id], |row| {
                Ok(UnderwaterPoint {
                    time: row.get(0)?,
                    drawdown_pct: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, rusqlite::Error>>()?;

        let markers = markers_from_trades(&trades);
        Ok(Some(BacktestRunDetail {
            run,
            trades,
            drawdown_events,
            equity,
            underwater,
            markers,
        }))
    }
}

fn markers_from_trades(trades: &[BacktestTrade]) -> Vec<StrategyMarker> {
    let mut markers = Vec::with_capacity(trades.len() * 2);
    for trade in trades {
        markers.push(StrategyMarker {
            time: trade.entry_time,
            side: trade.side.clone(),
            price: trade.entry_price,
            kind: "entry".into(),
        });
        if trade.status == "closed" {
            markers.push(StrategyMarker {
                time: trade.exit_time,
                side: trade.side.clone(),
                price: trade.exit_price,
                kind: "exit".into(),
            });
        }
    }
    markers
}

/// 读取单条运行的水下曲线（对比用）。
///
/// # Errors
/// SQLite 失败时返回 [`crate::app::state::StateError`]。
pub(crate) fn load_underwater(store: &StateStore, id: &str) -> StateResult<Vec<UnderwaterPoint>> {
    let conn = store.conn()?;
    let mut stmt = conn.prepare(
        "SELECT time_ms, drawdown_pct FROM backtest_underwater WHERE run_id = ?1 ORDER BY seq",
    )?;
    let rows = stmt.query_map(params![id], |row| {
        Ok(UnderwaterPoint {
            time: row.get(0)?,
            drawdown_pct: row.get(1)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}
