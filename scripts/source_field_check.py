#!/usr/bin/env python3
"""AC-D02 源文件逐字段对账。

对 12 个周期各取一个或两个月度归档，用 `binance-core` 读取器解析，再与 zip
内 CSV 原文逐行逐字段比较。字段映射与时间口径写在脚本文档与
`docs/data.md`，比对结果输出差异报告。

映射（现货 K 线归档，12 字段、无表头）：

| 源列 | 解析字段 | 处理 |
|---|---|---|
| 0 `open_time` | `open_time` | 微秒归一为毫秒（≥ 1e15 时整除 1000） |
| 1 `open` | `open` | 十进制 → f64 |
| 2 `high` | `high` | 同上 |
| 3 `low` | `low` | 同上 |
| 4 `close` | `close` | 同上 |
| 5 `volume` | `volume` | 同上，非负校验 |
| 6 `close_time` | `close_time` | 微秒归一为毫秒 |
| 7 `quote_volume` | `quote_volume` | 十进制 → f64 |
| 8 `count` | `count` | 整数 |
| 9 `taker_buy_volume` | `taker_buy_volume` | 十进制 → f64 |
| 10 `taker_buy_quote_volume` | `taker_buy_quote_volume` | 十进制 → f64 |
| 11 `ignore` | `ignore` | 整数 |

时区：归档时间戳是 UTC 毫秒（2025-01-01 起现货改为 UTC 微秒），`close_time`
等于该 bar 最后一毫秒，即 `open_time + 周期 - 1`。

用法（在仓库根目录）：
    python3 scripts/source_field_check.py --pair BTCUSDT \
        --reader /data/data-reader/target/release/binance-core \
        --archive-root /data/data/binance \
        --month 2024-01 --month 2025-01 --out /tmp/d02.json

输出：`--out` 指向的 JSON 报告；任一行不一致时退出码 1。
"""

from __future__ import annotations

import argparse
import calendar
import json
import struct
import subprocess
import sys
import tempfile
import zipfile
from datetime import UTC, datetime
from pathlib import Path

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
FIELDS = {
    "open_time": 0,
    "open": 1,
    "high": 2,
    "low": 3,
    "close": 4,
    "volume": 5,
    "close_time": 6,
    "quote_volume": 7,
    "count": 8,
    "taker_buy_volume": 9,
    "taker_buy_quote_volume": 10,
    "ignore": 11,
}
FLOATS = {
    "open",
    "high",
    "low",
    "close",
    "volume",
    "quote_volume",
    "taker_buy_volume",
    "taker_buy_quote_volume",
}
INTS = {"count", "ignore"}
MICROSECOND_THRESHOLD = 10**15


def normalize_time(raw: int) -> int:
    return raw // 1_000 if raw >= MICROSECOND_THRESHOLD else raw


def same_float(left: float, right: float) -> bool:
    return struct.pack("<d", left) == struct.pack("<d", right)


def run_reader(
    reader: str, root: str, pair: str, interval: str, month: str
) -> tuple[dict, str]:
    year, mon = (int(part) for part in month.split("-"))
    last = calendar.monthrange(year, mon)[1]
    config = (
        f'root = "{root}"\n'
        'market = "spot"\n'
        'dataset = "klines"\n'
        f'symbol = "{pair}"\n'
        f'interval = "{interval}"\n'
        f'start_date = "{month}-01"\n'
        f'end_date = "{month}-{last:02d}"\n'
    )
    with tempfile.NamedTemporaryFile("w", suffix=".toml", delete=False) as handle:
        handle.write(config)
        path = handle.name
    try:
        result = subprocess.run(
            [reader, "read", "-c", path],
            capture_output=True,
            text=True,
            check=False,
            timeout=300,
        )
    finally:
        Path(path).unlink(missing_ok=True)
    if result.returncode != 0:
        raise SystemExit(f"reader failed for {interval} {month}: {result.stderr[-800:]}")
    return json.loads(result.stdout), result.stderr.strip()


def compare_file(
    archive_root: str, pair: str, interval: str, month: str, columns: dict
) -> dict[str, object]:
    year, mon = (int(part) for part in month.split("-"))
    path = (
        Path(archive_root)
        / "data/spot/monthly/klines"
        / pair
        / interval
        / f"{pair}-{interval}-{year}-{mon:02d}.zip"
    )
    with zipfile.ZipFile(path) as archive:
        names = [name for name in archive.namelist() if name.endswith(".csv")]
        if len(names) != 1:
            raise SystemExit(f"{path}: expected one CSV entry, found {len(names)}")
        raw_lines = archive.read(names[0]).decode().splitlines()

    step = INTERVALS[interval]
    month_start = int(
        datetime(year, mon, 1, tzinfo=UTC).timestamp() * 1000
    )
    rows = len(next(iter(columns.values())))
    problems: list[str] = []
    gaps = 0
    previous: int | None = None
    for index, line in enumerate(raw_lines):
        if index >= rows:
            problems.append(f"row {index}: parser stopped at {rows} rows")
            break
        fields = line.split(",")
        if len(fields) != 12:
            problems.append(f"row {index}: {len(fields)} source fields")
            continue
        for name, index_field in FIELDS.items():
            raw = fields[index_field].strip()
            parsed = columns[name][index]
            if name in INTS:
                expected = int(raw)
            elif name == "open_time" or name == "close_time":
                expected = normalize_time(int(raw))
            else:
                expected = float(raw)
            if name in FLOATS:
                ok = same_float(float(parsed), expected)
            else:
                ok = int(parsed) == expected
            if not ok:
                problems.append(
                    f"row {index} {name}: source={raw} parsed={parsed}"
                )
                break
        open_time = normalize_time(int(fields[0]))
        close_time = normalize_time(int(fields[6]))
        expected_close = open_time + step - 1
        if close_time != expected_close:
            problems.append(
                f"row {index} close_time: {close_time} != {expected_close}"
            )
        if open_time % step != 0:
            problems.append(f"row {index} open_time not aligned: {open_time}")
        if index == 0 and open_time != month_start:
            problems.append(
                f"first open_time {open_time} != month start {month_start}"
            )
        if previous is not None and open_time - previous != step:
            gaps += 1
        previous = open_time
        if len(problems) >= 10:
            break

    if rows != len(raw_lines):
        problems.append(f"parser rows {rows} != source rows {len(raw_lines)}")

    return {
        "file": path.name,
        "source_rows": len(raw_lines),
        "parsed_rows": rows,
        "interval_step_ms": step,
        "gaps": gaps,
        "ok": not problems,
        "problems": problems[:10],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive-root", default="/data/data/binance")
    parser.add_argument("--reader", required=True)
    parser.add_argument("--pair", default="BTCUSDT")
    parser.add_argument("--month", action="append", default=[])
    parser.add_argument(
        "--sample-rows", type=int, default=1000, help="正式抽样行数（全量另计）"
    )
    parser.add_argument("--out", type=Path, default=Path("/tmp/d02.json"))
    args = parser.parse_args()
    months = args.month or ["2024-01", "2025-01"]

    results = []
    total_rows = 0
    for interval in INTERVALS:
        for month in months:
            parsed, log = run_reader(
                args.reader, args.archive_root, args.pair, interval, month
            )
            outcome = compare_file(
                args.archive_root, args.pair, interval, month, parsed["columns"]
            )
            outcome.update({"interval": interval, "month": month})
            if not outcome["ok"]:
                outcome["reader_log"] = log[-500:]
            results.append(outcome)
            total_rows += outcome["source_rows"]
            status = "ok" if outcome["ok"] else "FAIL"
            print(
                f"{interval:>3} {month} rows={outcome['source_rows']:>6} "
                f"gaps={outcome['gaps']} {status}",
                flush=True,
            )

    failures = [item for item in results if not item["ok"]]
    report = {
        "generated_at": datetime.now(UTC).isoformat(timespec="seconds"),
        "pair": args.pair,
        "months": months,
        "intervals": list(INTERVALS),
        "mapping": {name: index for name, index in FIELDS.items()},
        "time_normalization": "raw >= 1e15 ? raw / 1000 : raw (UTC ms)",
        "rows_compared": total_rows,
        "formal_sample_rows": args.sample_rows,
        "results": results,
        "ok": not failures,
    }
    args.out.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"rows_compared={total_rows} failures={len(failures)} report={args.out}")
    return 0 if not failures else 1


if __name__ == "__main__":
    sys.exit(main())
