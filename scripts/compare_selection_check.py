#!/usr/bin/env python3
"""AC-C02/C05 对比指标与批量选择演练。

流程：用临时服务提交 100 组参数扫描 → 核对 100 个运行的参数映射与完成状态 →
从结果中勾选 10 组做对比，逐项核对对比页指标与详情页一致、参数与运行 ID
无错位 → 验证 11 组被拒绝 → 导出 10 组 CSV，核对行数、列名与指标。

用法（在仓库根目录）：
    python3 scripts/compare_selection_check.py \
        --binary target/release/tradingview \
        --data-dir /data/data --work-dir /tmp/c05 --port 3016

输出：`<work-dir>/result.json`；任一检查失败时退出码 1。
"""

from __future__ import annotations

import argparse
import csv
import io
import json
import os
import shutil
import signal
import subprocess
import sys
import time
import urllib.error
import urllib.request
from datetime import UTC, datetime
from pathlib import Path

START_MS = 1_704_067_200_000  # 2024-01-01T00:00:00Z
END_MS = 1_711_929_600_000  # 2024-04-01T00:00:00Z
FAST = [2, 3, 4, 5, 6, 7, 8, 9, 10, 11]
SLOW = [20, 25, 30, 35, 40, 45, 50, 60, 70, 80]
CSV_COLUMNS = [
    "run_id",
    "label",
    "pair",
    "timeframe",
    "strategy",
    "strategy_version",
    "params",
    "fee_bps",
    "slippage_bps",
    "initial_balance",
    "start_ms",
    "end_ms",
    "bars",
    "data_first_ms",
    "data_last_ms",
    "num_trades",
    "win_rate",
    "total_return_pct",
    "sharpe_ratio",
    "sortino_ratio",
    "max_drawdown_pct",
    "avg_drawdown_pct",
    "drawdown_count",
    "longest_drawdown_days",
    "longest_recovery_days",
    "drawdown_time_pct",
    "profit_factor",
    "expectancy",
    "final_equity",
]
CSV_METRICS = CSV_COLUMNS[15:]


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
            if not raw:
                return response.status, None
            text = raw.decode()
            if path.split("?", 1)[0].endswith(".csv"):
                return response.status, text
            return response.status, json.loads(text)
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


def wait_batch(base: str, ids: set[str], timeout: float = 900) -> dict[str, dict]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        status, payload = api(base, "/api/backtests?limit=200")
        if status != 200:
            raise RuntimeError(f"list returned {status}: {payload}")
        runs = {run["id"]: run for run in payload["data"] if run["id"] in ids}
        if len(runs) == len(ids) and all(
            run["status"] in {"succeeded", "failed"} for run in runs.values()
        ):
            return runs
        time.sleep(1.0)
    raise RuntimeError(f"batch did not finish in {timeout}s")


class Drill:
    def __init__(self, args: argparse.Namespace) -> None:
        self.args = args
        self.work = Path(args.work_dir)
        self.config = self.work / "service.toml"
        self.log = self.work / "service.log"
        self.base = f"http://127.0.0.1:{args.port}"

    def write_config(self) -> None:
        args = self.args
        self.config.write_text(
            f"""host = "127.0.0.1"
port = {args.port}
data_dir = "{args.data_dir}"
db_path = "{self.work / "state.db"}"
frontend_dir = "{args.frontend_dir}"

[backtest]
enabled = true
python_bin = "{args.python_bin}"
worker_dir = "{args.worker_dir}"
output_dir = "{self.work / "backtests"}"
max_concurrent = 2
max_compare_runs = 10
max_grid_runs = 100
max_curve_points = 2000
keep_equity = true
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
            body = {
                "pair": self.args.pair,
                "timeframe": self.args.timeframe,
                "param_grid": {"fast": FAST, "slow": SLOW},
                "fee_bps": 10.0,
                "slippage_bps": 5.0,
                "initial_balance": 10000.0,
                "start_ms": START_MS,
                "end_ms": END_MS,
            }
            status, payload = api(self.base, "/api/backtests", "POST", body)
            if status != 202:
                raise RuntimeError(f"submit returned {status}: {payload}")
            submitted = payload["data"]["runs"]
            batch_id = payload["data"]["batch_id"]

            expected = {(fast, slow) for fast in FAST for slow in SLOW}
            actual = {(run["params"]["fast"], run["params"]["slow"]) for run in submitted}
            mapping = {
                "runs": len(submitted),
                "batch_size_ok": len(submitted) == 100,
                "unique_ids": len({run["id"] for run in submitted}) == 100,
                "same_batch": all(run["batch_id"] == batch_id for run in submitted),
                "grid_mapping_exact": actual == expected,
                "params_text_consistent": all(
                    run["params_text"]
                    == f"fast={run['params']['fast']},slow={run['params']['slow']}"
                    for run in submitted
                ),
            }
            for name, passed in mapping.items():
                if not passed:
                    problems.append(name)

            ids = [run["id"] for run in submitted]
            finished = wait_batch(self.base, set(ids))
            statuses = {
                "all_succeeded": all(
                    run["status"] == "succeeded" for run in finished.values()
                ),
                "all_have_metrics": all(
                    run["metrics"] is not None for run in finished.values()
                ),
            }
            for name, passed in statuses.items():
                if not passed:
                    problems.append(name)

            # 勾选 10 组：取两端各 5 组，覆盖不同 fast/slow。
            selected = ids[:5] + ids[-5:]
            status, payload = api(
                self.base, "/api/backtests/compare", "POST", {"ids": selected}
            )
            if status != 200:
                raise RuntimeError(f"compare returned {status}: {payload}")
            compare = payload["data"]
            details = {
                run_id: api(self.base, f"/api/backtests/{run_id}")[1]["data"]["run"]
                for run_id in selected
            }
            order_ok = [run["id"] for run in compare["runs"]] == selected
            series_ok = [series["run_id"] for series in compare["series"]] == selected
            metrics_equal = all(
                run["metrics"] == details[run["id"]]["metrics"]
                for run in compare["runs"]
            )
            labels_ok = all(
                series["params_text"] == details[series["run_id"]]["params_text"]
                and f"[{series['run_id'][:8]}]" in series["label"]
                for series in compare["series"]
            )
            compare_checks = {
                "ten_series": len(compare["series"]) == 10,
                "request_order_preserved": order_ok and series_ok,
                "metrics_equal_to_detail": metrics_equal,
                "labels_match_params": labels_ok,
                "no_metadata_warnings": compare["warnings"] == [],
                "params_differ_without_warning": len(
                    {(run["params"]["fast"], run["params"]["slow"]) for run in compare["runs"]}
                )
                > 1,
            }
            for name, passed in compare_checks.items():
                if not passed:
                    problems.append(name)

            status, payload = api(
                self.base, "/api/backtests/compare", "POST", {"ids": ids[:11]}
            )
            limit_checks = {
                "eleven_rejected": status == 400,
                "limit_message": isinstance(payload, str) and "at most 10" in payload,
            }
            for name, passed in limit_checks.items():
                if not passed:
                    problems.append(name)

            status, text = api(
                self.base,
                "/api/backtests/compare.csv?ids=" + ",".join(selected),
            )
            if status != 200:
                raise RuntimeError(f"csv returned {status}: {text}")
            rows = list(csv.DictReader(io.StringIO(text)))
            header = next(csv.reader(io.StringIO(text)))
            csv_run_ids = [row["run_id"] for row in rows]
            csv_metrics_ok = True
            for row in rows:
                metrics = details[row["run_id"]]["metrics"] or {}
                for column in CSV_METRICS:
                    want = metrics.get(column)
                    cell = row[column]
                    if want is None:
                        if cell != "":
                            csv_metrics_ok = False
                    elif not cell or abs(float(cell) - float(want)) > 1e-9:
                        csv_metrics_ok = False
            csv_checks = {
                "csv_rows": len(rows) == 10,
                "csv_header": header == CSV_COLUMNS,
                "csv_ids_in_order": csv_run_ids == selected,
                "csv_metrics_match": csv_metrics_ok,
            }
            for name, passed in csv_checks.items():
                if not passed:
                    problems.append(name)

            result = {
                "generated_at": datetime.now(UTC).isoformat(timespec="seconds"),
                "pair": self.args.pair,
                "timeframe": self.args.timeframe,
                "batch_id": batch_id,
                "grid": {"fast": FAST, "slow": SLOW},
                "mapping": mapping,
                "statuses": statuses,
                "compare": {
                    "selected": selected,
                    "series": len(compare["series"]),
                    "union_points": compare["union_points"],
                    **compare_checks,
                },
                "limit": limit_checks,
                "csv": csv_checks,
                "ok": not problems,
                "problems": problems,
            }
            (self.work / "result.json").write_text(
                json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8"
            )
            print(
                f"batch={batch_id} runs={len(submitted)} compare=10 "
                f"csv_rows={len(rows)} ok={result['ok']}",
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
    parser.add_argument("--work-dir", type=Path, default=Path("/tmp/c05"))
    parser.add_argument("--port", type=int, default=3016)
    parser.add_argument("--pair", default="BTCUSDT")
    parser.add_argument("--timeframe", default="1h")
    parser.add_argument("--python-bin", default="/data/vbt/.venv/bin/python")
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
