-- 回测运行、交易、回撤事件与降采样曲线。
-- 原始全量净值不默认入库；曲线由 worker 降采样后写入。

CREATE TABLE IF NOT EXISTS backtest_runs (
    id                TEXT PRIMARY KEY NOT NULL,
    batch_id          TEXT NOT NULL,
    status            TEXT NOT NULL CHECK (status IN ('queued', 'running', 'succeeded', 'failed')),
    market_type       TEXT NOT NULL,
    pair              TEXT NOT NULL,
    timeframe         TEXT NOT NULL,
    strategy          TEXT NOT NULL,
    strategy_version  TEXT NOT NULL,
    params_json       TEXT NOT NULL,
    params_text       TEXT NOT NULL,
    fee_bps           REAL NOT NULL,
    slippage_bps      REAL NOT NULL,
    initial_balance   REAL NOT NULL,
    start_ms          INTEGER,
    end_ms            INTEGER,
    created_at        INTEGER NOT NULL,
    started_at        INTEGER,
    finished_at       INTEGER,
    error             TEXT,
    log_path          TEXT,
    bars              INTEGER,
    data_first_ms     INTEGER,
    data_last_ms      INTEGER,
    engine            TEXT,
    worker_version    TEXT,
    metrics_json      TEXT,
    trades_truncated  INTEGER NOT NULL DEFAULT 0,
    retention         TEXT NOT NULL DEFAULT 'downsampled'
);

CREATE INDEX IF NOT EXISTS idx_backtest_runs_created
    ON backtest_runs (created_at DESC);
CREATE INDEX IF NOT EXISTS idx_backtest_runs_status
    ON backtest_runs (status);
CREATE INDEX IF NOT EXISTS idx_backtest_runs_batch
    ON backtest_runs (batch_id);

CREATE TABLE IF NOT EXISTS backtest_trades (
    run_id       TEXT NOT NULL REFERENCES backtest_runs (id) ON DELETE CASCADE,
    seq          INTEGER NOT NULL,
    entry_time   INTEGER NOT NULL,
    exit_time    INTEGER NOT NULL,
    side         TEXT NOT NULL,
    status       TEXT NOT NULL,
    entry_price  REAL,
    exit_price   REAL,
    qty          REAL,
    pnl          REAL,
    pnl_pct      REAL,
    bars_held    INTEGER NOT NULL,
    PRIMARY KEY (run_id, seq)
);

CREATE TABLE IF NOT EXISTS backtest_drawdown_events (
    run_id         TEXT NOT NULL REFERENCES backtest_runs (id) ON DELETE CASCADE,
    seq            INTEGER NOT NULL,
    peak_time      INTEGER NOT NULL,
    trough_time    INTEGER NOT NULL,
    recovery_time  INTEGER,
    depth_pct      REAL NOT NULL,
    duration_ms    INTEGER NOT NULL,
    recovery_ms    INTEGER,
    recovered      INTEGER NOT NULL,
    PRIMARY KEY (run_id, seq)
);

CREATE TABLE IF NOT EXISTS backtest_equity (
    run_id    TEXT NOT NULL REFERENCES backtest_runs (id) ON DELETE CASCADE,
    seq       INTEGER NOT NULL,
    time_ms   INTEGER NOT NULL,
    value     REAL NOT NULL,
    PRIMARY KEY (run_id, seq)
);

CREATE TABLE IF NOT EXISTS backtest_underwater (
    run_id       TEXT NOT NULL REFERENCES backtest_runs (id) ON DELETE CASCADE,
    seq          INTEGER NOT NULL,
    time_ms      INTEGER NOT NULL,
    drawdown_pct REAL NOT NULL,
    PRIMARY KEY (run_id, seq)
);
