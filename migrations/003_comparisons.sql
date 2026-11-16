-- 命名保存的回测对比。
--
-- run_ids 是运行 ID 的 JSON 数组（2..=max_compare_runs），不建外键：
-- 运行被删除后对比仍可列出，读取时由后端统计 missing_runs。

CREATE TABLE IF NOT EXISTS backtest_comparisons (
    id         TEXT PRIMARY KEY NOT NULL,
    name       TEXT NOT NULL UNIQUE,
    run_ids    TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS backtest_comparisons_updated_at_idx
    ON backtest_comparisons (updated_at DESC);
