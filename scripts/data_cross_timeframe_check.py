#!/usr/bin/env python3
"""AC-D05 跨周期一致性检查。

用 1m 归档聚合出抽样的 1h 与 1d bar，与原生周期逐根比较：

- 每根 1h 必须恰好覆盖 60 根 1m，1d 恰好 1,440 根；
- open 取首根、high 取最大、low 取最小、close 取末根，要求与原生逐值相等；
- volume 求和，相对误差 ≤ 0.1%。

抽样在整年上均匀取点，缺口会直接表现为覆盖不足。脚本同时统计 1m/1h/1d
归档在区间内的缺失根数，用于发现下载缺口。

用法（在仓库根目录）：
    python/.venv/bin/python scripts/data_cross_timeframe_check.py \
        --data-dir /data/data --pair BTCUSDT --start 2024-01-01 \
        --end 2025-01-01 --hours 100 --days 10 --out /tmp/d05.json

输出：`--out` 指向的 JSON 报告；任一抽样窗口不完整或 OHLC/成交量不达标时退出码 1。
"""

from __future__ import annotations

import argparse
import json
import sys
from datetime import UTC, datetime
from pathlib import Path

import numpy as np

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "python"))

from backtest_worker.data import LoadedKlines, load_klines  # noqa: E402

STEP_MS = {
    "1m": 60_000,
    "1h": 3_600_000,
    "1d": 86_400_000,
}
VOLUME_TOLERANCE = 1e-3


def columns(loaded: LoadedKlines) -> tuple[np.ndarray, ...]:
    frame = loaded.frame
    return (
        frame.index.as_unit("ms").asi8,
        frame["open"].to_numpy(),
        frame["high"].to_numpy(),
        frame["low"].to_numpy(),
        frame["close"].to_numpy(),
        frame["volume"].to_numpy(),
    )


def sample_indices(count: int, wanted: int) -> list[int]:
    if count < wanted:
        raise SystemExit(f"only {count} native bars, need {wanted}")
    picks = np.linspace(0, count - 1, wanted).round().astype(int)
    return sorted({int(index) for index in picks})


def check_samples(
    label: str,
    native: tuple[np.ndarray, ...],
    minute: tuple[np.ndarray, ...],
    wanted: int,
) -> dict[str, object]:
    native_ts, n_open, n_high, n_low, n_close, n_volume = native
    m_ts, m_open, m_high, m_low, m_close, m_volume = minute
    step = STEP_MS[label]
    problems: list[str] = []
    ohlc_mismatches = 0
    volume_mismatches = 0
    volume_max_rel = 0.0
    incomplete: list[str] = []
    checked = 0

    for index in sample_indices(len(native_ts), wanted):
        start = int(native_ts[index])
        end = start + step
        lo = int(np.searchsorted(m_ts, start, side="left"))
        hi = int(np.searchsorted(m_ts, end, side="left"))
        expected = step // STEP_MS["1m"]
        if hi - lo != expected:
            incomplete.append(f"{start}: {hi - lo}/{expected} 1m bars")
            continue
        checked += 1
        values = {
            "open": (m_open[lo], n_open[index]),
            "high": (m_high[lo:hi].max(), n_high[index]),
            "low": (m_low[lo:hi].min(), n_low[index]),
            "close": (m_close[hi - 1], n_close[index]),
        }
        for name, (derived, reference) in values.items():
            if float(derived) != float(reference):
                ohlc_mismatches += 1
                problems.append(
                    f"{label} {start} {name}: 1m-derived={derived} native={reference}"
                )
        native_volume = float(n_volume[index])
        derived_volume = float(m_volume[lo:hi].sum())
        if native_volume == 0.0:
            relative = 0.0 if derived_volume == 0.0 else 1.0
        else:
            relative = abs(derived_volume - native_volume) / abs(native_volume)
        volume_max_rel = max(volume_max_rel, relative)
        if relative > VOLUME_TOLERANCE:
            volume_mismatches += 1
            problems.append(
                f"{label} {start} volume: derived={derived_volume} native={native_volume} rel={relative:.2e}"
            )

    if incomplete:
        problems.extend(f"{label} incomplete window {item}" for item in incomplete[:10])
    if checked != wanted:
        problems.append(f"{label} checked {checked} windows, expected {wanted}")

    return {
        "samples_requested": wanted,
        "samples_checked": checked,
        "incomplete_windows": len(incomplete),
        "incomplete_samples": incomplete[:10],
        "ohlc_mismatches": ohlc_mismatches,
        "volume_mismatches": volume_mismatches,
        "volume_max_rel_error": volume_max_rel,
        "problems": problems[:20],
    }


def coverage(loaded: LoadedKlines, step_ms: int) -> dict[str, object]:
    if loaded.rows == 0:
        return {"rows": 0, "expected": 0, "missing": 0}
    expected = (loaded.last_ms - loaded.first_ms) // step_ms + 1
    return {
        "rows": loaded.rows,
        "files": loaded.files,
        "first_time": loaded.first_ms,
        "last_time": loaded.last_ms,
        "expected": expected,
        "missing": expected - loaded.rows,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data-dir", default="/data/data")
    parser.add_argument("--pair", default="BTCUSDT")
    parser.add_argument("--start", default="2024-01-01")
    parser.add_argument("--end", default="2025-01-01")
    parser.add_argument("--hours", type=int, default=100)
    parser.add_argument("--days", type=int, default=10)
    parser.add_argument("--out", type=Path, default=Path("/tmp/d05.json"))
    args = parser.parse_args()

    start_ms = int(datetime.fromisoformat(args.start).replace(tzinfo=UTC).timestamp() * 1000)
    end_ms = int(datetime.fromisoformat(args.end).replace(tzinfo=UTC).timestamp() * 1000)

    minute_loaded = load_klines(args.data_dir, args.pair, "1m", start_ms, end_ms)
    hour_loaded = load_klines(args.data_dir, args.pair, "1h", start_ms, end_ms)
    day_loaded = load_klines(args.data_dir, args.pair, "1d", start_ms, end_ms)

    minute = columns(minute_loaded)
    hour = columns(hour_loaded)
    day = columns(day_loaded)

    problems: list[str] = []
    hour_result = check_samples("1h", hour, minute, args.hours)
    day_result = check_samples("1d", day, minute, args.days)
    problems.extend(hour_result["problems"])
    problems.extend(day_result["problems"])

    datasets = {
        "1m": coverage(minute_loaded, STEP_MS["1m"]),
        "1h": coverage(hour_loaded, STEP_MS["1h"]),
        "1d": coverage(day_loaded, STEP_MS["1d"]),
    }
    missing_minute = int(datasets["1m"]["missing"])
    if missing_minute:
        problems.append(f"1m archive is missing {missing_minute} bars")

    report = {
        "generated_at": datetime.now(UTC).isoformat(timespec="seconds"),
        "pair": args.pair,
        "start": args.start,
        "end": args.end,
        "volume_tolerance": VOLUME_TOLERANCE,
        "datasets": datasets,
        "checks": {"1h": hour_result, "1d": day_result},
        "ok": not problems,
        "problems": problems[:20],
    }
    args.out.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print(
        "1m rows={} missing={} | 1h checked={} ohlc={} vol_max={:.2e} | "
        "1d checked={} ohlc={} vol_max={:.2e} {}".format(
            datasets["1m"]["rows"],
            datasets["1m"]["missing"],
            hour_result["samples_checked"],
            hour_result["ohlc_mismatches"],
            hour_result["volume_max_rel_error"],
            day_result["samples_checked"],
            day_result["ohlc_mismatches"],
            day_result["volume_max_rel_error"],
            "ok" if not problems else "FAIL",
        )
    )
    for problem in problems[:10]:
        print(f"  - {problem}")
    print(f"report: {args.out}")
    return 0 if not problems else 1


if __name__ == "__main__":
    sys.exit(main())
