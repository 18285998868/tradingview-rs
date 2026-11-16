#!/usr/bin/env python3
"""AC-D06 全量导入基准：按成交额取前 N 个现货标的，下载 12 周期月档。

流程：从 Binance 取交易规则与 24 小时成交额，按成交额选出前 N 个 USDT 现货
标的 → 生成下载器配置 → 计时运行下载（自带 checksum 校验）→ 统计文件数、
字节数、失败数与每个标的的平均占用。

用法（在仓库根目录）：
    scripts/data_import_bench.py --downloader /data/data-downloader/target/release/binance-data-downloader \
        --output-dir /data/d06-bench --symbols 100 --concurrency 64 \
        --proxy http://127.0.0.1:1081 --out /tmp/d06.json

`--dry-run` 只生成配置并打印选中的标的；不访问下载器。
输出：`--out` 指向的 JSON 报告；下载失败时退出码 1。
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import time
import urllib.request
from datetime import UTC, datetime
from pathlib import Path

INTERVALS = [
    "1m",
    "3m",
    "5m",
    "15m",
    "30m",
    "1h",
    "2h",
    "4h",
    "6h",
    "8h",
    "12h",
    "1d",
]

# 杠杆代币与稳定币对不代表普通现货标的，排除后成交额排名更接近真实市场。
LEVERAGED = ("UP", "DOWN", "BULL", "BEAR")
STABLE_BASES = {
    "USDT",
    "USDC",
    "FDUSD",
    "TUSD",
    "DAI",
    "BUSD",
    "EUR",
    "AEUR",
    "USD1",
    "XUSD",
    "PAXG",
}


def api(path: str, proxy: str | None, timeout: int = 30) -> object:
    opener = (
        urllib.request.build_opener(
            urllib.request.ProxyHandler({"http": proxy, "https": proxy})
        )
        if proxy
        else urllib.request.build_opener()
    )
    request = urllib.request.Request(
        "https://api.binance.com" + path, headers={"user-agent": "data-import-bench/1"}
    )
    with opener.open(request, timeout=timeout) as response:
        return json.loads(response.read())


def pick_symbols(proxy: str | None, count: int) -> list[str]:
    info = api("/api/v3/exchangeInfo", proxy)
    tickers = {
        item["symbol"]: float(item["quoteVolume"])
        for item in api("/api/v3/ticker/24hr", proxy)
    }
    candidates: list[tuple[float, str]] = []
    for item in info["symbols"]:
        symbol = item["symbol"]
        if item.get("status") != "TRADING":
            continue
        if not symbol.isascii() or not re.fullmatch(r"[A-Z0-9_]+", symbol):
            continue
        if item.get("quoteAsset") != "USDT" or not item.get("isSpotTradingAllowed"):
            continue
        if symbol.endswith(("USDT",)) and symbol[: -len("USDT")].endswith(LEVERAGED):
            continue
        if item.get("baseAsset") in STABLE_BASES:
            continue
        candidates.append((tickers.get(symbol, 0.0), symbol))
    candidates.sort(reverse=True)
    return [symbol for _, symbol in candidates[:count]]


def write_config(
    path: Path, symbols: list[str], output_dir: str, concurrency: int
) -> None:
    lines = [
        "# 由 scripts/data_import_bench.py 生成；AC-D06 固定数据集。",
        'market = "spot"',
        "symbol = [" + ", ".join(json.dumps(s) for s in symbols) + "]",
        f"output_dir = {json.dumps(output_dir)}",
        f"concurrency = {concurrency}",
        "",
        "[[dataset]]",
        'type = "klines"',
        'periods = ["monthly"]',
        "intervals = [" + ", ".join(json.dumps(i) for i in INTERVALS) + "]",
        "",
    ]
    path.write_text("\n".join(lines), encoding="utf-8")


def tree_bytes(root: Path) -> tuple[int, int, int]:
    total = 0
    zip_bytes = 0
    files = 0
    for dirpath, _, names in os.walk(root):
        for name in names:
            path = Path(dirpath) / name
            size = path.stat().st_size
            total += size
            files += 1
            if name.endswith(".zip"):
                zip_bytes += size
    return total, zip_bytes, files


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--downloader", required=True)
    parser.add_argument("--output-dir", required=True)
    parser.add_argument("--symbols", type=int, default=100)
    parser.add_argument("--concurrency", type=int, default=64)
    parser.add_argument("--proxy", default="")
    parser.add_argument("--config", type=Path, default=None)
    parser.add_argument("--log", type=Path, default=None)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    symbols = pick_symbols(args.proxy or None, args.symbols)
    if len(symbols) < args.symbols:
        raise SystemExit(f"only {len(symbols)} symbols matched, need {args.symbols}")

    config = args.config or Path("/tmp/d06-config.toml")
    write_config(config, symbols, args.output_dir, args.concurrency)
    log_path = args.log or Path("/tmp/d06-download.log")

    report: dict[str, object] = {
        "generated_at": datetime.now(UTC).isoformat(timespec="seconds"),
        "symbols": symbols,
        "intervals": INTERVALS,
        "concurrency": args.concurrency,
        "output_dir": args.output_dir,
        "config": str(config),
    }
    if args.dry_run:
        print(json.dumps(report, ensure_ascii=False, indent=2))
        return 0

    started = time.monotonic()
    env = dict(os.environ)
    if args.proxy:
        # 归档域名直连更快；只有 api.binance.com 起始日查询需要代理。
        env.update(
            {
                "HTTP_PROXY": args.proxy,
                "HTTPS_PROXY": args.proxy,
                "NO_PROXY": "data.binance.vision",
            }
        )
    with log_path.open("wb") as log:
        process = subprocess.run(
            [args.downloader, "-c", str(config)],
            stdout=log,
            stderr=subprocess.STDOUT,
            env=env,
            check=False,
        )
    elapsed = time.monotonic() - started
    report["exit_code"] = process.returncode
    report["elapsed_secs"] = round(elapsed, 3)
    report["download_env"] = {
        "proxy": args.proxy,
        "no_proxy": "data.binance.vision" if args.proxy else "",
    }

    cache = Path(args.output_dir) / "binance" / ".cache" / "last-run.json"
    if cache.exists():
        report["downloader"] = json.loads(cache.read_text())

    root = Path(args.output_dir) / "binance" / "data" / "spot" / "monthly" / "klines"
    total, zip_bytes, files = tree_bytes(root)
    report["storage"] = {
        "bytes": total,
        "gib": round(total / 1024**3, 3),
        "gb": round(total / 1000**3, 3),
        "zip_bytes": zip_bytes,
        "zip_gib": round(zip_bytes / 1024**3, 3),
        "files": files,
        "per_symbol_mib": round(total / max(len(symbols), 1) / 1024**2, 2),
    }
    report["files_per_symbol"] = {}
    if root.is_dir():
        for symbol_dir in sorted(root.iterdir()):
            if symbol_dir.is_dir():
                report["files_per_symbol"][symbol_dir.name] = sum(
                    1 for _ in symbol_dir.rglob("*.zip")
                )

    args.out.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps({k: report[k] for k in ("elapsed_secs", "exit_code", "storage")}, ensure_ascii=False))
    print(f"report: {args.out}, log: {log_path}")
    return 0 if process.returncode == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
