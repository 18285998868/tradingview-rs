#!/usr/bin/env python3
"""AC-R05 保留策略与孤儿数据演练。

流程：用 `keep_equity = false` 启动临时服务 → 跑一组真实回测 → 核对权益表为空、
水下曲线与指标仍在 → 重试生成新运行并确认旧运行完整 → 删除两个运行并检查
子表无残留、`pragma foreign_key_check` 为空。

用法（在仓库根目录）：
    python3 scripts/retention_check.py \
        --binary target/release/tradingview \
        --data-dir /data/data --work-dir /tmp/r05 --port 3015

输出：`<work-dir>/result.json`；任一检查失败时退出码 1。
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import signal
import sqlite3
import subprocess
import sys
import time
import urllib.error
import urllib.request
from contextlib import closing
from datetime import UTC, datetime
from pathlib import Path

START_MS = 1_704_067_200_000  # 2024-01-01T00:00:00Z
END_MS = 1_711_929_600_000  # 2024-04-01T00:00:00Z
CHILD_TABLES = [
    "backtest_trades",
    "backtest_drawdown_events",
    "backtest_equity",
    "backtest_underwater",
]


def api(
    base: str,
    path: str,
    method: str = "GET",
    body: dict | None = None,
) -> tuple[int, object]:
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(
        base + path,
        data=data,
        method=method,
        headers={"content-type": "application/json"} if data else {},
    )
    try:
        with urllib.request.urlopen(request, timeout=180) as response:
            raw = response.read()
            return response.status, json.loads(raw.decode()) if raw else None
    except urllib.error.HTTPError as exc:
        return exc.code, exc.read().decode(errors="replace")


def wait_health(base: str) -> None:
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(base + "/api/health", timeout=1) as response:
                if response.status == 200:
                    return
        except OSError:
            time.sleep(0.1)
    raise RuntimeError("service did not become healthy")


def wait_run(base: str, run_id: str, timeout: float = 600) -> dict:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        status, payload = api(base, f"/api/backtests/{run_id}")
        if status != 200:
            raise RuntimeError(f"detail {run_id} returned {status}: {payload}")
        detail = payload["data"]
        run = detail["run"]
        if run["status"] == "succeeded":
            return detail
        if run["status"] == "failed":
            raise RuntimeError(f"run {run_id} failed: {run.get('error')}")
        time.sleep(0.5)
    raise RuntimeError(f"run {run_id} did not finish in {timeout}s")


def table_counts(db_path: Path, run_id: str) -> dict[str, int]:
    counts = {"backtest_runs": 0}
    with closing(sqlite3.connect(db_path, timeout=10)) as con:
        counts["backtest_runs"] = con.execute(
            "select count(*) from backtest_runs where id = ?", (run_id,)
        ).fetchone()[0]
        for table in CHILD_TABLES:
            counts[table] = con.execute(
                f"select count(*) from {table} where run_id = ?", (run_id,)
            ).fetchone()[0]
    return counts


def foreign_key_issues(db_path: Path) -> list[tuple]:
    with closing(sqlite3.connect(db_path, timeout=10)) as con:
        return con.execute("pragma foreign_key_check").fetchall()


class Drill:
    def __init__(self, args: argparse.Namespace) -> None:
        self.args = args
        self.work = Path(args.work_dir)
        self.db = self.work / "state.db"
        self.config = self.work / "service.toml"
        self.log = self.work / "service.log"
        self.base = f"http://127.0.0.1:{args.port}"

    def write_config(self) -> None:
        args = self.args
        self.config.write_text(
            f"""host = "127.0.0.1"
port = {args.port}
data_dir = "{args.data_dir}"
db_path = "{self.db}"
frontend_dir = "{args.frontend_dir}"

[backtest]
enabled = true
python_bin = "{args.python_bin}"
worker_dir = "{args.worker_dir}"
output_dir = "{self.work / "backtests"}"
max_concurrent = 1
max_curve_points = 2000
keep_equity = false
""",
            encoding="utf-8",
        )

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
        wait_health(self.base)
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

    def run(self) -> int:
        shutil.rmtree(self.work, ignore_errors=True)
        self.work.mkdir(parents=True)
        self.write_config()

        problems: list[str] = []
        process = self.start()
        try:
            status, payload = api(self.base, "/api/backtests/config")
            if status != 200:
                raise RuntimeError(f"config returned {status}: {payload}")
            config_keep_equity = payload["data"]["limits"].get("keep_equity")
            if config_keep_equity is not False:
                problems.append(f"config keep_equity = {config_keep_equity}, want false")

            body = {
                "pair": self.args.pair,
                "timeframe": self.args.timeframe,
                "params": {"fast": 10, "slow": 30},
                "fee_bps": 10.0,
                "slippage_bps": 5.0,
                "initial_balance": 10000.0,
                "start_ms": START_MS,
                "end_ms": END_MS,
            }
            status, payload = api(self.base, "/api/backtests", "POST", body)
            if status != 202:
                raise RuntimeError(f"submit returned {status}: {payload}")
            run = payload["data"]["runs"][0]
            run_id = run["id"]

            detail = wait_run(self.base, run_id)
            summary = detail["run"]
            metrics = summary["metrics"] or {}
            counts_before = table_counts(self.db, run_id)
            first_underwater = len(detail["underwater"])

            checks = {
                "config_keep_equity_false": config_keep_equity is False,
                "retention_underwater_only": summary["retention"] == "underwater_only",
                "equity_empty": detail["equity"] == [],
                "underwater_present": first_underwater > 0,
                "metrics_present": summary["metrics"] is not None,
                "drawdown_metric_present": metrics.get("max_drawdown_pct") is not None,
                "equity_rows_zero": counts_before["backtest_equity"] == 0,
                "underwater_rows_positive": counts_before["backtest_underwater"] > 0,
                "run_meta_traceable": bool(summary.get("engine"))
                and bool(summary.get("worker_version"))
                and summary.get("bars") is not None,
            }
            for name, passed in checks.items():
                if not passed:
                    problems.append(name)

            # 重试：新运行独立落库，旧运行的曲线与事件不受影响。
            status, payload = api(self.base, f"/api/backtests/{run_id}/retry", "POST")
            if status != 202:
                raise RuntimeError(f"retry returned {status}: {payload}")
            retry_id = payload["data"]["runs"][0]["id"]
            retry_detail = wait_run(self.base, retry_id)
            old_detail = api(self.base, f"/api/backtests/{run_id}")[1]["data"]
            retry_checks = {
                "retry_new_id": retry_id != run_id,
                "retry_succeeded": (retry_detail["run"]["metrics"] or {}).get("num_trades")
                is not None,
                "old_rows_preserved": len(old_detail["underwater"]) == first_underwater,
                "retry_underwater_present": len(retry_detail["underwater"]) > 0,
                "no_orphans_after_retry": foreign_key_issues(self.db) == [],
            }
            for name, passed in retry_checks.items():
                if not passed:
                    problems.append(name)

            # 删除两个运行：运行行与四张子表都不留残行。
            deleted: dict[str, dict[str, int]] = {}
            for target in (run_id, retry_id):
                status, _ = api(self.base, f"/api/backtests/{target}", "DELETE")
                if status != 204:
                    problems.append(f"delete {target} returned {status}")
                deleted[target] = table_counts(self.db, target)
            after_delete = {
                "runs_removed": all(
                    counts["backtest_runs"] == 0 for counts in deleted.values()
                ),
                "children_removed": all(
                    count == 0
                    for counts in deleted.values()
                    for name, count in counts.items()
                    if name != "backtest_runs"
                ),
                "no_orphans_after_delete": foreign_key_issues(self.db) == [],
            }
            for name, passed in after_delete.items():
                if not passed:
                    problems.append(name)

            summary_out = {
                "generated_at": datetime.now(UTC).isoformat(timespec="seconds"),
                "pair": self.args.pair,
                "timeframe": self.args.timeframe,
                "start_ms": START_MS,
                "end_ms": END_MS,
                "config_keep_equity": config_keep_equity,
                "run": {
                    "id": run_id,
                    "retention": summary["retention"],
                    "bars": summary["bars"],
                    "engine": summary["engine"],
                    "underwater_points": first_underwater,
                    "drawdown_events": len(detail["drawdown_events"]),
                    "trades": len(detail["trades"]),
                    "max_drawdown_pct": metrics.get("max_drawdown_pct"),
                },
                "checks": checks,
                "retry": {"new_id": retry_id, **retry_checks},
                "delete": {
                    "counts": deleted,
                    **after_delete,
                },
                "ok": not problems,
                "problems": problems,
            }
            (self.work / "result.json").write_text(
                json.dumps(summary_out, ensure_ascii=False, indent=2), encoding="utf-8"
            )
            print(
                f"retention={summary['retention']} equity={len(detail['equity'])} "
                f"underwater={first_underwater} retry={retry_id} "
                f"ok={summary_out['ok']}",
                flush=True,
            )
        finally:
            self.stop(process)

        if problems:
            print("problems: " + "; ".join(problems), file=sys.stderr)
            return 1
        print(f"report: {self.work / 'result.json'}")
        return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--data-dir", default="/data/data")
    parser.add_argument("--work-dir", type=Path, default=Path("/tmp/r05"))
    parser.add_argument("--port", type=int, default=3015)
    parser.add_argument("--pair", default="BTCUSDT")
    parser.add_argument("--timeframe", default="1h")
    parser.add_argument(
        "--python-bin",
        default=str(Path(__file__).resolve().parent.parent / "python/.venv/bin/python"),
    )
    parser.add_argument("--worker-dir", default=str(Path(__file__).resolve().parent.parent / "python"))
    parser.add_argument(
        "--frontend-dir",
        default=str(Path(__file__).resolve().parent.parent / "frontend" / "dist"),
    )
    parser.add_argument("--cwd", default=str(Path(__file__).resolve().parent.parent))
    args = parser.parse_args()
    return Drill(args).run()


if __name__ == "__main__":
    sys.exit(main())
