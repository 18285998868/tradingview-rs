#!/usr/bin/env python3
"""AC-P07 数据库快照恢复演练。

流程：从来源库在线备份快照 → 用快照副本启动临时服务 → 记录抽样行情与最近
若干回测运行 → 写入标记布局 → 停服 → 用快照覆盖数据库 → 重启 → 复核。

用法（在仓库根目录）：
    python3 scripts/snapshot_restore_check.py \
        --source-db /tmp/tradingview-verify.db \
        --work-dir /tmp/p07 --data-dir /data/data

输出：`<work-dir>/result.json`；任一比对不一致时退出码 1。
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import signal
import sqlite3
import subprocess
import sys
import time
import urllib.request
from contextlib import closing
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent


def sqlite_backup(source: str, target: str) -> None:
    with closing(sqlite3.connect(source)) as src, closing(sqlite3.connect(target)) as dst:
        src.backup(dst)


def sqlite_read(path: str, query: str, attempts: int = 6) -> list[tuple]:
    """停服瞬间可能撞上 WAL 回收窗口，重试几次。"""
    for attempt in range(attempts):
        try:
            with closing(sqlite3.connect(path, timeout=10)) as con:
                return con.execute(query).fetchall()
        except sqlite3.OperationalError:
            if attempt == attempts - 1:
                raise
            time.sleep(0.5)
    raise RuntimeError("unreachable")


def table_counts(path: str) -> dict[str, int]:
    tables = [
        row[0]
        for row in sqlite_read(
            path, "select name from sqlite_master where type='table' order by name"
        )
    ]
    return {table: sqlite_read(path, f"select count(*) from {table}")[0][0] for table in tables}


def integrity(path: str) -> str:
    return sqlite_read(path, "pragma integrity_check")[0][0]


def digest(value: object) -> str:
    raw = json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(raw.encode()).hexdigest()


class Drill:
    def __init__(self, args: argparse.Namespace) -> None:
        self.args = args
        self.work = Path(args.work_dir)
        self.snapshot = self.work / "snapshot.db"
        self.restore = self.work / "restore.db"
        self.config = self.work / "service.toml"
        self.log = self.work / "service.log"
        self.base = f"http://127.0.0.1:{args.port}"

    def api(self, path: str, method: str = "GET", body: dict | None = None) -> dict:
        data = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(
            self.base + path,
            data=data,
            method=method,
            headers={"content-type": "application/json"} if data else {},
        )
        with urllib.request.urlopen(request, timeout=120) as response:
            return json.loads(response.read().decode())

    def wait_health(self) -> None:
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            try:
                with urllib.request.urlopen(self.base + "/api/health", timeout=1) as response:
                    if response.status == 200:
                        return
            except OSError:
                time.sleep(0.1)
        raise RuntimeError("service did not become healthy")

    def start(self) -> subprocess.Popen:
        process = subprocess.Popen(
            [self.args.binary],
            cwd=self.args.cwd,
            env={**os.environ, "TRADINGVIEW_CONFIG": str(self.config)},
            stdin=subprocess.DEVNULL,
            stdout=open(self.log, "ab"),
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        self.wait_health()
        return process

    @staticmethod
    def stop(process: subprocess.Popen) -> None:
        if process.poll() is None:
            process.send_signal(signal.SIGTERM)
            try:
                process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)

    def sample(self, before: dict | None = None) -> dict:
        args = self.args
        klines = self.api(
            f"/api/klines?market_type={args.sample_market}"
            f"&pair={args.sample_pair}&timeframe={args.sample_timeframe}"
            f"&limit={args.sample_limit}&before={args.sample_before}"
        )["data"]["klines"]
        runs = self.api("/api/backtests?limit=200")["data"]
        succeeded = [run for run in runs if run["status"] == "succeeded"][: args.runs]
        details = {run["id"]: self.api(f"/api/backtests/{run['id']}")["data"] for run in succeeded}
        layouts = self.api("/api/layouts")["data"]["layouts"]
        result = {
            "klines": klines,
            "klines_digest": digest(klines),
            "run_ids": [run["id"] for run in succeeded],
            "details_digest": {run_id: digest(detail) for run_id, detail in details.items()},
            "layout_names": sorted(layout["name"] for layout in layouts),
        }
        if before:
            result["klines_equal"] = result["klines"] == before["klines"]
            result["details_equal"] = result["details_digest"] == before["details_digest"]
        return result

    def write_config(self) -> None:
        args = self.args
        self.config.write_text(
            f"""host = "127.0.0.1"
port = {args.port}
data_dir = "{args.data_dir}"
db_path = "{self.restore}"
frontend_dir = "{args.frontend_dir}"

[backtest]
enabled = true
python_bin = "{args.python_bin}"
worker_dir = "{args.worker_dir}"
output_dir = "{self.work / "backtests"}"
max_concurrent = 1
keep_equity = true
""",
            encoding="utf-8",
        )

    def run(self) -> int:
        args = self.args
        shutil.rmtree(self.work, ignore_errors=True)
        self.work.mkdir(parents=True)
        started = time.monotonic()
        sqlite_backup(args.source_db, str(self.snapshot))
        snapshot_secs = time.monotonic() - started
        snapshot_bytes = self.snapshot.stat().st_size
        shutil.copy2(self.snapshot, self.restore)
        self.write_config()

        snapshot_counts = table_counts(str(self.snapshot))
        snapshot_integrity = integrity(str(self.snapshot))

        process = self.start()
        try:
            before = self.sample()
            print(
                f"before: klines={len(before['klines'])} runs={len(before['run_ids'])} "
                f"layouts={before['layout_names']}",
                flush=True,
            )
            marker = self.api(
                "/api/layouts",
                "POST",
                {
                    "name": args.marker,
                    "market_type": args.sample_market,
                    "pair": args.sample_pair,
                    "timeframe": args.sample_timeframe,
                    "payload": {
                        "market_type": args.sample_market,
                        "pair": args.sample_pair,
                        "timeframe": args.sample_timeframe,
                        "indicators": [],
                        "viewport": None,
                    },
                },
            )
            marker_id = marker["data"]["id"]
            with closing(sqlite3.connect(self.restore)) as con:
                marker_present = con.execute(
                    "select count(*) from layouts where id = ?", (marker_id,)
                ).fetchone()[0]
            print(f"marker inserted: {marker_id} present={marker_present}", flush=True)
        finally:
            self.stop(process)

        # 恢复：先写临时文件再原子替换，避免覆盖仍被引用的 inode。
        for suffix in ("-wal", "-shm"):
            try:
                os.remove(str(self.restore) + suffix)
            except FileNotFoundError:
                pass
        staging = Path(str(self.restore) + ".new")
        shutil.copy2(self.snapshot, staging)
        os.replace(staging, self.restore)
        time.sleep(0.5)

        process = self.start()
        try:
            after = self.sample(before)
            print(
                f"after: klines={len(after['klines'])} runs={len(after['run_ids'])} "
                f"layouts={after['layout_names']} marker_listed="
                f"{args.marker in after['layout_names']}",
                flush=True,
            )
        finally:
            self.stop(process)

        restored_counts = table_counts(str(self.restore))
        summary = {
            "snapshot_secs": round(snapshot_secs, 2),
            "snapshot_bytes": snapshot_bytes,
            "source_bytes": os.path.getsize(args.source_db),
            "snapshot_integrity": snapshot_integrity,
            "restored_integrity": integrity(str(self.restore)),
            "snapshot_counts": snapshot_counts,
            "restored_counts": restored_counts,
            "counts_equal": snapshot_counts == restored_counts,
            "klines": len(before["klines"]),
            "klines_digest_equal": before["klines_digest"] == after["klines_digest"],
            "runs": len(before["run_ids"]),
            "run_ids_equal": before["run_ids"] == after["run_ids"],
            "details_equal": before["details_digest"] == after["details_digest"],
            "marker_removed": args.marker not in after["layout_names"],
            "run_ids": before["run_ids"],
        }
        (self.work / "result.json").write_text(
            json.dumps(summary, ensure_ascii=False, indent=2), encoding="utf-8"
        )
        print("SUMMARY " + json.dumps(summary, ensure_ascii=False), flush=True)

        checks = [
            summary["klines_digest_equal"],
            summary["details_equal"],
            summary["run_ids_equal"],
            summary["counts_equal"],
            summary["marker_removed"],
            summary["snapshot_integrity"] == "ok",
            summary["restored_integrity"] == "ok",
        ]
        return 0 if all(checks) else 1


def main() -> int:
    parser = argparse.ArgumentParser(description="AC-P07 数据库快照恢复演练")
    parser.add_argument("--source-db", required=True, help="来源 SQLite 状态库")
    parser.add_argument("--work-dir", default="/tmp/p07-check", help="演练工作目录")
    parser.add_argument("--data-dir", required=True, help="服务使用的行情数据目录")
    parser.add_argument("--port", type=int, default=3013, help="临时服务端口")
    parser.add_argument("--binary", default=str(REPO / "target/release/tradingview"))
    parser.add_argument("--cwd", default=str(REPO), help="服务工作目录")
    parser.add_argument("--frontend-dir", default=str(REPO / "frontend/dist"))
    parser.add_argument("--python-bin", default="python3", help="worker Python 解释器")
    parser.add_argument("--worker-dir", default=str(REPO / "python"))
    parser.add_argument("--sample-market", default="futures")
    parser.add_argument("--sample-pair", default="BTCUSDT")
    parser.add_argument("--sample-timeframe", default="1m")
    parser.add_argument("--sample-limit", type=int, default=100)
    parser.add_argument("--sample-before", type=int, default=1688166000000)
    parser.add_argument("--runs", type=int, default=10, help="比对的成功运行数")
    parser.add_argument("--marker", default="P07 restore marker")
    args = parser.parse_args()
    return Drill(args).run()


if __name__ == "__main__":
    sys.exit(main())
