#!/usr/bin/env python3
"""AC-D01 12 周期抽样对账 Binance 官方 K 线。

对每个周期取固定 1000 根窗口：本地用回测 worker 的数据加载器读取现货归档，
远端用 Binance 现货 REST `/api/v3/klines` 取同一窗口。OHLC 要求逐值完全相等，
成交量相对误差 ≤ 0.1%。

用法（在仓库根目录、装有 pandas/numpy 的解释器下）：
    python/.venv/bin/python scripts/data_official_check.py \
        --data-dir /data/data --pair BTCUSDT --start 2024-01-01 \
        --proxy http://127.0.0.1:1081 --out /tmp/d01.json

输出：`--out` 指向的 JSON 报告；任一周期不达标时退出码 1。
"""

from __future__ import annotations

import argparse
import json
import sys
import time
import urllib.parse
import urllib.request
from datetime import UTC, datetime
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "python"))

from backtest_worker.data import load_klines  # noqa: E402

INTERVALS = {
    "1m": 60_000,
    "3m": 180_000,
    "5m": 300_000,
    "15m": 900_000,
    "30m": 1_800_000,
    "1h": 3_600_000,
    "2h": 7_200_000,
    "4h": 14_400_000,
    "6h": 21_600_000,
    "8h": 28_800_000,
    "12h": 43_200_000,
    "1d": 86_400_000,
}
VOLUME_TOLERANCE = 1e-3


def fetch_official(
    symbol: str, interval: str, start_ms: int, limit: int, proxy: str | None
) -> list[list[str]]:
    query = urllib.parse.urlencode(
        {
            "symbol": symbol,
            "interval": interval,
            "startTime": start_ms,
            "limit": limit,
        }
    )
    url = "https://api.binance.com/api/v3/klines?" + query
    handlers = (
        [urllib.request.ProxyHandler({"http": proxy, "https": proxy})] if proxy else []
    )
    opener = urllib.request.build_opener(*handlers)
    last: Exception | None = None
    for attempt in range(4):
        try:
            request = urllib.request.Request(
                url, headers={"user-agent": "source-field-check/1"}
            )
            with opener.open(request, timeout=30) as response:
                return json.loads(response.read())
        except Exception as exc:  # 网络抖动或限频，退避后重试
            last = exc
            time.sleep(1 + attempt)
    raise SystemExit(f"official klines failed: {last}")


def compare(
    interval: str, local, official: list[list[str]], start_ms: int, bars: int
) -> dict[str, object]:
    frame = local.frame
    # pandas 3 的 DatetimeIndex 内部单位可能是 µs，统一换成毫秒再取整数。
    stamps = frame.index.as_unit("ms").asi8.tolist()
    expected = [start_ms + i * INTERVALS[interval] for i in range(bars)]
    official_stamps = [int(row[0]) for row in official]

    problems: list[str] = []
    if len(official) != bars:
        problems.append(f"official rows {len(official)} != {bars}")
    if len(stamps) != bars:
        problems.append(f"local rows {len(stamps)} != {bars}")
    if official_stamps != expected:
        problems.append("official timestamps are not the expected window")
    if stamps != expected:
        problems.append("local timestamps are not the expected window")

    ohlc_mismatch = 0
    volume_max_rel = 0.0
    volume_mismatch = 0
    for index, row in enumerate(official[: len(stamps)]):
        if index >= len(stamps):
            break
        values = frame.iloc[index]
        for offset, name in enumerate(("open", "high", "low", "close"), start=1):
            if float(row[offset]) != float(values[name]):
                ohlc_mismatch += 1
                problems.append(
                    f"{interval} bar {index} {name}: local={values[name]} official={row[offset]}"
                )
                break
        official_volume = float(row[5])
        local_volume = float(values["volume"])
        if official_volume == 0.0:
            relative = 0.0 if local_volume == 0.0 else 1.0
        else:
            relative = abs(local_volume - official_volume) / abs(official_volume)
        volume_max_rel = max(volume_max_rel, relative)
        if relative > VOLUME_TOLERANCE:
            volume_mismatch += 1
            problems.append(
                f"{interval} bar {index} volume: local={local_volume} official={official_volume} rel={relative:.2e}"
            )

    return {
        "interval": interval,
        "bars": len(stamps),
        "official_bars": len(official),
        "first_time": stamps[0] if stamps else None,
        "last_time": stamps[-1] if stamps else None,
        "ohlc_mismatches": ohlc_mismatch,
        "volume_max_rel_error": volume_max_rel,
        "volume_mismatches": volume_mismatch,
        "files": local.files,
        "ok": not problems and ohlc_mismatch == 0 and volume_mismatch == 0,
        "problems": problems[:10],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data-dir", default="/data/data")
    parser.add_argument("--pair", default="BTCUSDT")
    parser.add_argument("--start", default="2024-01-01", help="UTC 起始日期")
    parser.add_argument("--bars", type=int, default=1000)
    parser.add_argument("--proxy", default="")
    parser.add_argument("--out", type=Path, default=Path("/tmp/d01.json"))
    args = parser.parse_args()

    day = datetime.fromisoformat(args.start).replace(tzinfo=UTC)
    start_ms = int(day.timestamp() * 1000)
    rows = []
    for interval in INTERVALS:
        step = INTERVALS[interval]
        local = load_klines(
            args.data_dir,
            args.pair,
            interval,
            start_ms,
            start_ms + args.bars * step,
        )
        official = fetch_official(
            args.pair, interval, start_ms, args.bars, args.proxy or None
        )
        result = compare(interval, local, official, start_ms, args.bars)
        rows.append(result)
        status = "ok" if result["ok"] else "FAIL"
        print(
            f"{interval:>3} bars={result['bars']:>5} ohlc_mismatch={result['ohlc_mismatches']} "
            f"vol_max_rel={result['volume_max_rel_error']:.2e} {status}",
            flush=True,
        )

    report = {
        "generated_at": datetime.now(UTC).isoformat(timespec="seconds"),
        "pair": args.pair,
        "start": args.start,
        "bars": args.bars,
        "volume_tolerance": VOLUME_TOLERANCE,
        "results": rows,
        "ok": all(row["ok"] for row in rows),
    }
    args.out.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"report: {args.out}")
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
