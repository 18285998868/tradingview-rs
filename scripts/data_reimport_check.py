#!/usr/bin/env python3
"""AC-D03 重复导入幂等性检查。

在空目录连续跑两次同一个下载配置，核对：

- zip 集合、字节数、sha256 完全一致，第二次不重写已有文件；
- 用 `binance-core` 读取同一区间，两次行数完全相同；
- 每次运行都写 `binance/.cache/last-run.json`，`finished_at` 可查且第二次更新。

用法（在仓库根目录）：
    python3 scripts/data_reimport_check.py \
        --downloader /data/data-downloader/target/release/binance-data-downloader \
        --reader /data/data-reader/target/release/binance-core \
        --work-dir /tmp/d03 --clean --out /tmp/d03.json

输出：`--out` 指向的 JSON 报告；任一幂等性断言不符时退出码 1。
"""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import subprocess
import sys
import time
from datetime import UTC, datetime
from pathlib import Path


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def snapshot(root: Path) -> dict[str, dict[str, object]]:
    files: dict[str, dict[str, object]] = {}
    if not root.is_dir():
        return files
    for path in sorted(root.rglob("*.zip")):
        stat = path.stat()
        files[str(path.relative_to(root))] = {
            "bytes": stat.st_size,
            "mtime_ns": stat.st_mtime_ns,
            "sha256": sha256(path),
        }
    return files


def run_downloader(downloader: str, config: Path) -> dict[str, object]:
    started = time.monotonic()
    result = subprocess.run(
        [downloader, "-c", str(config)],
        capture_output=True,
        text=True,
        check=False,
        timeout=1800,
    )
    elapsed = time.monotonic() - started
    return {
        "exit_code": result.returncode,
        "elapsed_secs": round(elapsed, 3),
        "stderr_tail": result.stderr.strip().splitlines()[-5:],
    }


def write_config(
    path: Path, root: Path, symbols: list[str], intervals: list[str], start: str, end: str
) -> None:
    path.write_text(
        "\n".join(
            [
                "# 由 scripts/data_reimport_check.py 生成；AC-D03 固定小数据集。",
                'market = "spot"',
                "symbol = [" + ", ".join(json.dumps(s) for s in symbols) + "]",
                f"start = {json.dumps(start)}",
                f"end = {json.dumps(end)}",
                f"output_dir = {json.dumps(str(root))}",
                "concurrency = 8",
                "",
                "[[dataset]]",
                'type = "klines"',
                'periods = ["monthly"]',
                "intervals = [" + ", ".join(json.dumps(i) for i in intervals) + "]",
                "",
            ]
        ),
        encoding="utf-8",
    )


def read_rows(
    reader: str, root: Path, symbol: str, interval: str, start: str, end: str
) -> int:
    config = (
        f'root = "{root}"\n'
        'market = "spot"\n'
        'dataset = "klines"\n'
        f'symbol = "{symbol}"\n'
        f'interval = "{interval}"\n'
        f'start_date = "{start}"\n'
        f'end_date = "{end}"\n'
    )
    path = root.parent / f"read-{symbol}-{interval}.toml"
    path.write_text(config, encoding="utf-8")
    result = subprocess.run(
        [reader, "read", "-c", str(path)],
        capture_output=True,
        text=True,
        check=False,
        timeout=300,
    )
    if result.returncode != 0:
        raise SystemExit(
            f"reader failed for {symbol}/{interval}: {result.stderr.strip()[-500:]}"
        )
    return int(json.loads(result.stdout)["report"]["rows"])


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--downloader", required=True)
    parser.add_argument("--reader", required=True)
    parser.add_argument("--work-dir", type=Path, default=Path("/tmp/d03"))
    parser.add_argument("--symbols", default="BTCUSDT,ETHUSDT")
    parser.add_argument("--intervals", default="1h,1d")
    parser.add_argument("--start", default="2024-01-01")
    parser.add_argument("--end", default="2024-03-31")
    parser.add_argument("--clean", action="store_true")
    parser.add_argument("--out", type=Path, default=Path("/tmp/d03.json"))
    args = parser.parse_args()

    work = args.work_dir
    if work.exists():
        if not args.clean:
            raise SystemExit(f"{work} exists; pass --clean to replace it")
        shutil.rmtree(work)
    work.mkdir(parents=True)

    symbols = [item.strip().upper() for item in args.symbols.split(",") if item.strip()]
    intervals = [item.strip() for item in args.intervals.split(",") if item.strip()]
    archive_root = work / "binance/data/spot/monthly/klines"
    config = work / "download.toml"
    write_config(config, work, symbols, intervals, args.start, args.end)

    problems: list[str] = []
    runs: list[dict[str, object]] = []
    snapshots: list[dict[str, dict[str, object]]] = []
    summaries: list[dict[str, object]] = []
    rows: list[dict[str, int]] = []

    for index in range(2):
        run = run_downloader(args.downloader, config)
        runs.append(run)
        if run["exit_code"] != 0:
            problems.append(f"run {index + 1} exited {run['exit_code']}")
        snapshots.append(snapshot(archive_root))
        before = {
            f"{symbol}/{interval}": read_rows(
                args.reader, work / "binance", symbol, interval, args.start, args.end
            )
            for symbol in symbols
            for interval in intervals
        }
        rows.append(before)
        summary_path = work / "binance/.cache/last-run.json"
        if summary_path.exists():
            summary = json.loads(summary_path.read_text())
            summaries.append(
                {
                    "status": summary.get("status"),
                    "started_at": summary.get("started_at"),
                    "finished_at": summary.get("finished_at"),
                    "totals": summary.get("totals"),
                }
            )
        else:
            problems.append(f"run {index + 1} did not write last-run.json")
            summaries.append({})

    first, second = snapshots
    new_files = sorted(set(second) - set(first))
    missing_files = sorted(set(first) - set(second))
    checksum_mismatches = sorted(
        name
        for name in set(first) & set(second)
        if first[name]["sha256"] != second[name]["sha256"]
    )
    rewritten = sorted(
        name
        for name in set(first) & set(second)
        if first[name]["mtime_ns"] != second[name]["mtime_ns"]
    )
    if new_files:
        problems.append(f"second run added {len(new_files)} files")
    if missing_files:
        problems.append(f"second run removed {len(missing_files)} files")
    if checksum_mismatches:
        problems.append(f"{len(checksum_mismatches)} checksums changed")
    if rewritten:
        problems.append(f"{len(rewritten)} existing files were rewritten")
    if rows[0] != rows[1]:
        problems.append("row counts changed between runs")

    updated_time = bool(
        summaries[0].get("finished_at")
        and summaries[1].get("finished_at")
        and summaries[0]["finished_at"] != summaries[1]["finished_at"]
    )
    if not updated_time:
        problems.append("last-run.json finished_at did not change")
    if any(summary.get("status") != "ok" for summary in summaries):
        problems.append("a run summary is not ok")

    total_bytes = sum(int(item["bytes"]) for item in second.values())
    report = {
        "generated_at": datetime.now(UTC).isoformat(timespec="seconds"),
        "downloader": args.downloader,
        "reader": args.reader,
        "config": str(config),
        "start": args.start,
        "end": args.end,
        "symbols": symbols,
        "intervals": intervals,
        "runs": runs,
        "summaries": summaries,
        "files": {
            "count": len(second),
            "bytes": total_bytes,
            "mib": round(total_bytes / 1024**2, 3),
        },
        "rows": rows,
        "idempotency": {
            "new_files": new_files,
            "missing_files": missing_files,
            "checksum_mismatches": checksum_mismatches,
            "rewritten_files": rewritten,
            "rows_equal": rows[0] == rows[1],
            "finished_at_updated": updated_time,
        },
        "ok": not problems,
        "problems": problems,
    }
    args.out.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print(
        "files={} bytes={} rows={} rewritten={} checksum_mismatch={} {} report={}".format(
            len(second),
            total_bytes,
            rows[1],
            len(rewritten),
            len(checksum_mismatches),
            "ok" if not problems else "FAIL",
            args.out,
        )
    )
    for problem in problems:
        print(f"  - {problem}")
    return 0 if not problems else 1


if __name__ == "__main__":
    sys.exit(main())
