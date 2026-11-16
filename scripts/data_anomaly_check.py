#!/usr/bin/env python3
"""AC-D04 数据质量异常注入检查。

构造一个月度归档，注入已知异常后用 `binance-core` 读取，核对异常是否被
计数、采样或以可读错误拒绝，而不是静默丢弃：

- 缺失 bar：删掉 2 根 1h bar，读取报告的 `gaps` 应为 1 段，脚本按首尾时间
  推算缺失根数为 2；
- OHLC 逻辑异常：`high < low`，`bad_rows = "skip"` 时计入 `rows_skipped`
  并保留样例；
- 负成交量：同样计入 `rows_skipped`；
- 重复时间戳：合并后无法排序，`skip` 也必须失败，错误里带重复时间。

用法（在仓库根目录）：
    python3 scripts/data_anomaly_check.py \
        --reader /data/data-reader/target/release/binance-core \
        --work-dir /tmp/d04 --out /tmp/d04.json

输出：`--out` 指向的 JSON 报告；任一断言不符时退出码 1。
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import zipfile
from datetime import UTC, datetime
from pathlib import Path

STEP_MS = 3_600_000
BASE_MS = int(datetime(2024, 1, 1, tzinfo=UTC).timestamp() * 1000)
SYMBOL = "BTCUSDT"
INTERVAL = "1h"


def kline(
    index: int,
    price: float,
    *,
    high: float | None = None,
    low: float | None = None,
    volume: float = 10.0,
) -> str:
    open_time = BASE_MS + index * STEP_MS
    close_time = open_time + STEP_MS - 1
    open_price = price
    close_price = price + 10.0
    high = close_price + 5.0 if high is None else high
    low = open_price - 5.0 if low is None else low
    return (
        f"{open_time},{open_price:.8f},{high:.8f},{low:.8f},{close_price:.8f},"
        f"{volume:.8f},{close_time},1000.0,10,5.0,500.0,0"
    )


def write_archive(path: Path, lines: list[str]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    csv_name = path.with_suffix(".csv").name
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as archive:
        archive.writestr(csv_name, "\n".join(lines) + "\n")


def write_config(path: Path, root: Path, bad_rows: str) -> None:
    path.write_text(
        "\n".join(
            [
                f'root = "{root}"',
                'market = "spot"',
                'dataset = "klines"',
                f'symbol = "{SYMBOL}"',
                f'interval = "{INTERVAL}"',
                'start_date = "2024-01-01"',
                'end_date = "2024-01-31"',
                f'bad_rows = "{bad_rows}"',
                "",
            ]
        ),
        encoding="utf-8",
    )


def run_reader(reader: str, config: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [reader, "read", "-c", str(config)],
        capture_output=True,
        text=True,
        check=False,
        timeout=120,
    )


def error_excerpt(result: subprocess.CompletedProcess[str]) -> str:
    text = (result.stderr or result.stdout).strip().splitlines()
    return "\n".join(text[-6:])


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reader", required=True)
    parser.add_argument("--work-dir", type=Path, default=Path("/tmp/d04"))
    parser.add_argument("--out", type=Path, default=Path("/tmp/d04.json"))
    args = parser.parse_args()

    work = args.work_dir
    if work.exists():
        shutil.rmtree(work)
    root = work / "binance"
    archive = root / "data/spot/monthly/klines" / SYMBOL / INTERVAL
    archive.mkdir(parents=True)

    # 正常 10 根，删掉第 3、4 根制造一个缺口。
    anomaly_rows = [
        kline(index, 42_000.0 + index * 10) for index in (0, 1, 2, 5, 6, 7, 8, 9)
    ]
    # 第 10 根 high < low；第 11 根成交量为负。
    anomaly_rows.append(kline(10, 42_100.0, high=42_000.0, low=42_200.0))
    anomaly_rows.append(kline(11, 42_110.0, volume=-1.0))
    write_archive(archive / f"{SYMBOL}-{INTERVAL}-2024-01.zip", anomaly_rows)

    skip_config = work / "skip.toml"
    fail_config = work / "fail.toml"
    write_config(skip_config, root, "skip")
    write_config(fail_config, root, "fail")

    problems: list[str] = []

    skip_run = run_reader(args.reader, skip_config)
    skip_report: dict[str, object] | None = None
    if skip_run.returncode != 0:
        problems.append("skip policy failed: " + error_excerpt(skip_run))
    else:
        payload = json.loads(skip_run.stdout)
        skip_report = payload["report"]
        expected = {
            "rows": 8,
            "rows_decoded": 8,
            "rows_skipped": 2,
            "gaps": 1,
            "bad_rows": "skip",
        }
        for key, value in expected.items():
            if skip_report.get(key) != value:
                problems.append(
                    f"skip report {key}={skip_report.get(key)!r}, expected {value!r}"
                )
        first = skip_report.get("first_time")
        last = skip_report.get("last_time")
        if isinstance(first, int) and isinstance(last, int):
            missing = (last - first) // STEP_MS + 1 - int(skip_report["rows"])
            if missing != 2:
                problems.append(f"derived missing bars={missing}, expected 2")
        else:
            problems.append("skip report has no first_time/last_time")
        samples = skip_report.get("skipped_samples") or []
        if len(samples) != 2:
            problems.append(f"skipped_samples={len(samples)}, expected 2")
        elif not any("invalid OHLC" in str(item) for item in samples):
            problems.append("skipped_samples does not mention invalid OHLC")
        elif not any("negative" in str(item) for item in samples):
            problems.append("skipped_samples does not mention negative volume")

    fail_run = run_reader(args.reader, fail_config)
    if fail_run.returncode == 0:
        problems.append("fail policy accepted the damaged archive")
    elif "invalid OHLC" not in error_excerpt(fail_run):
        problems.append("fail policy error does not name invalid OHLC: " + error_excerpt(fail_run))

    # 重复时间戳单独构造：正常 10 根，再追加第 0 根的副本。
    duplicate_rows = [kline(index, 42_000.0 + index * 10) for index in range(10)]
    duplicate_rows.append(duplicate_rows[0])
    write_archive(archive / f"{SYMBOL}-{INTERVAL}-2024-01.zip", duplicate_rows)
    duplicate_run = run_reader(args.reader, skip_config)
    if duplicate_run.returncode == 0:
        problems.append("duplicate open_time was silently accepted")
    elif "duplicate or misordered" not in error_excerpt(duplicate_run):
        problems.append(
            "duplicate error is not explicit: " + error_excerpt(duplicate_run)
        )

    report = {
        "generated_at": datetime.now(UTC).isoformat(timespec="seconds"),
        "reader": args.reader,
        "fixture": str(archive / f"{SYMBOL}-{INTERVAL}-2024-01.zip"),
        "injected": {
            "missing_bars": 2,
            "invalid_ohlc_rows": 1,
            "negative_volume_rows": 1,
            "duplicate_open_time_rows": 1,
        },
        "skip_counts": skip_report,
        "skip_error": None if skip_report else error_excerpt(skip_run),
        "fail_policy": {
            "exit_code": fail_run.returncode,
            "error": error_excerpt(fail_run),
        },
        "duplicate": {
            "exit_code": duplicate_run.returncode,
            "error": error_excerpt(duplicate_run),
        },
        "ok": not problems,
        "problems": problems,
    }
    args.out.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print(
        "skip rows={} skipped={} gaps={} fail_rc={} duplicate_rc={} {} report={}".format(
            (skip_report or {}).get("rows"),
            (skip_report or {}).get("rows_skipped"),
            (skip_report or {}).get("gaps"),
            fail_run.returncode,
            duplicate_run.returncode,
            "ok" if not problems else "FAIL",
            args.out,
        )
    )
    for problem in problems:
        print(f"  - {problem}")
    return 0 if not problems else 1


if __name__ == "__main__":
    sys.exit(main())
