#!/usr/bin/env python3
"""AC-D07 按标的与时间范围查询一年 1m 数据的 P95 基准。

查询路径复用回测 worker 的 `load_klines`：只扫描与 `[start, end)` 相交的
月/日归档，解析 CSV、归一时间戳并按范围过滤。脚本先做一次冷启动记录，
再预热若干次，最后计时 100 次，取 P95 与 2 秒阈值比较。

用法（在仓库根目录）：
    python/.venv/bin/python scripts/data_query_bench.py \
        --data-dir /data/data --pair BTCUSDT --start 2024-01-01 \
        --end 2025-01-01 --runs 100 --out /tmp/d07.json

输出：`--out` 指向的 JSON 报告；P95 超过阈值时退出码 1。
"""

from __future__ import annotations

import argparse
import json
import statistics
import sys
import time
from datetime import UTC, datetime
from pathlib import Path

import numpy as np

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "python"))

from backtest_worker.data import load_klines  # noqa: E402

THRESHOLD_MS = 2000.0


def timed_load(data_dir: str, pair: str, start_ms: int, end_ms: int) -> tuple[float, int, int]:
    started = time.perf_counter()
    loaded = load_klines(data_dir, pair, "1m", start_ms, end_ms)
    elapsed_ms = (time.perf_counter() - started) * 1000
    return elapsed_ms, loaded.rows, loaded.files


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data-dir", default="/data/data")
    parser.add_argument("--pair", default="BTCUSDT")
    parser.add_argument("--start", default="2024-01-01")
    parser.add_argument("--end", default="2025-01-01")
    parser.add_argument("--runs", type=int, default=100)
    parser.add_argument("--warmup", type=int, default=1)
    parser.add_argument("--threshold-ms", type=float, default=THRESHOLD_MS)
    parser.add_argument("--out", type=Path, default=Path("/tmp/d07.json"))
    args = parser.parse_args()

    start_ms = int(datetime.fromisoformat(args.start).replace(tzinfo=UTC).timestamp() * 1000)
    end_ms = int(datetime.fromisoformat(args.end).replace(tzinfo=UTC).timestamp() * 1000)

    cold_ms, rows, files = timed_load(args.data_dir, args.pair, start_ms, end_ms)
    expected_rows = (end_ms - start_ms) // 60_000
    problems: list[str] = []
    if rows != expected_rows:
        problems.append(f"query returned {rows} rows, expected {expected_rows}")

    for _ in range(args.warmup):
        timed_load(args.data_dir, args.pair, start_ms, end_ms)

    timings: list[float] = []
    for _ in range(args.runs):
        elapsed_ms, run_rows, run_files = timed_load(
            args.data_dir, args.pair, start_ms, end_ms
        )
        timings.append(elapsed_ms)
        if run_rows != rows or run_files != files:
            problems.append("a repeated query returned a different shape")
            break

    array = np.asarray(timings)
    p50 = float(np.percentile(array, 50))
    p95 = float(np.percentile(array, 95))
    p99 = float(np.percentile(array, 99))
    if p95 > args.threshold_ms:
        problems.append(f"P95 {p95:.1f} ms exceeds {args.threshold_ms:.0f} ms")

    report = {
        "generated_at": datetime.now(UTC).isoformat(timespec="seconds"),
        "pair": args.pair,
        "interval": "1m",
        "start": args.start,
        "end": args.end,
        "rows": rows,
        "expected_rows": expected_rows,
        "files": files,
        "cold_ms": round(cold_ms, 3),
        "warmup_runs": args.warmup,
        "runs": args.runs,
        "threshold_ms": args.threshold_ms,
        "ms": {
            "min": round(min(timings), 3),
            "mean": round(statistics.fmean(timings), 3),
            "p50": round(p50, 3),
            "p95": round(p95, 3),
            "p99": round(p99, 3),
            "max": round(max(timings), 3),
        },
        "timings_ms": [round(value, 3) for value in timings],
        "ok": not problems,
        "problems": problems,
    }
    args.out.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print(
        "rows={} files={} cold={:.1f}ms p50={:.1f} p95={:.1f} p99={:.1f} max={:.1f} {} report={}".format(
            rows,
            files,
            cold_ms,
            report["ms"]["p50"],
            report["ms"]["p95"],
            report["ms"]["p99"],
            report["ms"]["max"],
            "ok" if not problems else "FAIL",
            args.out,
        )
    )
    for problem in problems:
        print(f"  - {problem}")
    return 0 if not problems else 1


if __name__ == "__main__":
    sys.exit(main())
