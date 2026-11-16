"""CLI：读取请求 JSON，执行回测，写结果 JSON。"""

from __future__ import annotations

import argparse
import json
import math
import os
import sys
import tempfile
import traceback
from pathlib import Path
from typing import Any

from . import __version__
from .data import DataError, load_klines
from .engine import STRATEGY, STRATEGY_VERSION, WORKER_VERSION, run_dual_ma
from .metrics import PERIODS_PER_YEAR


def _number(request: dict[str, Any], key: str, default: float) -> float:
    value = request.get(key, default)
    if not isinstance(value, (int, float)) or not math.isfinite(float(value)):
        raise ValueError(f"{key} must be a finite number")
    return float(value)


def _optional_ms(request: dict[str, Any], key: str) -> int | None:
    value = request.get(key)
    if value is None:
        return None
    if not isinstance(value, int) or value < 0:
        raise ValueError(f"{key} must be a non-negative integer")
    return value


def validate_request(request: dict[str, Any]) -> dict[str, Any]:
    """校验并补齐请求默认值；失败抛 ValueError。"""

    if not isinstance(request, dict):
        raise ValueError("request must be a JSON object")
    market = request.get("market_type", "spot")
    if market != "spot":
        raise ValueError(f"only Binance spot is supported, got market_type={market!r}")
    strategy = request.get("strategy", STRATEGY)
    if strategy != STRATEGY:
        raise ValueError(f"unsupported strategy {strategy!r}, only {STRATEGY!r} is available")
    timeframe = request.get("timeframe")
    if timeframe not in PERIODS_PER_YEAR:
        raise ValueError(f"unsupported timeframe {timeframe!r}")
    pair = request.get("pair")
    if not isinstance(pair, str) or not pair or len(pair) > 32:
        raise ValueError("pair must be a non-empty string of 1..=32 characters")

    params_list = request.get("params_list")
    if not isinstance(params_list, list) or not params_list:
        raise ValueError("params_list must be a non-empty array")
    normalized: list[dict[str, int]] = []
    seen: set[tuple[int, int]] = set()
    for item in params_list:
        if not isinstance(item, dict):
            raise ValueError("each params entry must be an object")
        try:
            fast = int(item["fast"])
            slow = int(item["slow"])
        except (KeyError, TypeError, ValueError) as exc:
            raise ValueError("each params entry needs integer fast/slow") from exc
        if fast < 1 or slow < 2 or fast >= slow:
            raise ValueError(f"invalid params fast={fast}, slow={slow}")
        key = (fast, slow)
        if key in seen:
            raise ValueError(f"duplicate params fast={fast}, slow={slow}")
        seen.add(key)
        normalized.append({"fast": fast, "slow": slow})

    data_dir = request.get("data_dir")
    if not isinstance(data_dir, str) or not data_dir:
        raise ValueError("data_dir must be a non-empty string")
    if not Path(data_dir).is_dir():
        raise ValueError(f"data_dir is not a directory: {data_dir}")

    fee_bps = _number(request, "fee_bps", 0.0)
    slippage_bps = _number(request, "slippage_bps", 0.0)
    initial_balance = _number(request, "initial_balance", 10_000.0)
    if fee_bps < 0 or slippage_bps < 0:
        raise ValueError("fee_bps and slippage_bps must be >= 0")
    if initial_balance <= 0:
        raise ValueError("initial_balance must be > 0")
    max_curve_points = int(request.get("max_curve_points", 2000))
    max_trades = int(request.get("max_trades", 200_000))
    if max_curve_points < 10:
        raise ValueError("max_curve_points must be >= 10")
    if max_trades < 1:
        raise ValueError("max_trades must be >= 1")

    start_ms = _optional_ms(request, "start_ms")
    end_ms = _optional_ms(request, "end_ms")
    if start_ms is not None and end_ms is not None and start_ms >= end_ms:
        raise ValueError("start_ms must be < end_ms")

    return {
        "market_type": market,
        "pair": pair,
        "timeframe": timeframe,
        "strategy": strategy,
        "strategy_version": STRATEGY_VERSION,
        "params_list": normalized,
        "data_dir": data_dir,
        "fee_bps": fee_bps,
        "slippage_bps": slippage_bps,
        "initial_balance": initial_balance,
        "max_curve_points": max_curve_points,
        "max_trades": max_trades,
        "start_ms": start_ms,
        "end_ms": end_ms,
    }


def execute(request: dict[str, Any]) -> dict[str, Any]:
    """执行请求，返回 worker 结果。"""

    normalized = validate_request(request)
    try:
        import vectorbt as vbt

        vectorbt_version = getattr(vbt, "__version__", "unknown")
    except ImportError as exc:
        raise RuntimeError(
            "vectorbt is not installed; point backtest.python_bin at the venv that has vectorbt"
        ) from exc

    loaded = load_klines(
        normalized["data_dir"],
        normalized["pair"],
        normalized["timeframe"],
        normalized["start_ms"],
        normalized["end_ms"],
    )
    runs: list[dict[str, Any]] = []
    for params in normalized["params_list"]:
        print(
            f"running {normalized['pair']} {normalized['timeframe']} "
            f"fast={params['fast']} slow={params['slow']} bars={loaded.rows}",
            flush=True,
        )
        try:
            run = run_dual_ma(
                frame=loaded.frame,
                params=params,
                timeframe=normalized["timeframe"],
                fee_bps=normalized["fee_bps"],
                slippage_bps=normalized["slippage_bps"],
                initial_balance=normalized["initial_balance"],
                max_curve_points=normalized["max_curve_points"],
                max_trades=normalized["max_trades"],
            )
        except Exception as exc:  # 单组失败不拖垮整批，交由 Rust 标记该运行 failed
            traceback.print_exc()
            run = {
                "params": params,
                "params_text": f"fast={params['fast']},slow={params['slow']}",
                "bars": loaded.rows,
                "first_time": loaded.first_ms,
                "last_time": loaded.last_ms,
                "metrics": None,
                "trades": [],
                "trades_truncated": False,
                "drawdown_events": [],
                "equity": [],
                "underwater": [],
                "error": f"{type(exc).__name__}: {exc}",
            }
        runs.append(run)

    return {
        "worker_version": WORKER_VERSION,
        "engine": f"vectorbt {vectorbt_version}",
        "strategy": STRATEGY,
        "strategy_version": STRATEGY_VERSION,
        "pair": normalized["pair"],
        "timeframe": normalized["timeframe"],
        "fee_bps": normalized["fee_bps"],
        "slippage_bps": normalized["slippage_bps"],
        "initial_balance": normalized["initial_balance"],
        "start_ms": normalized["start_ms"],
        "end_ms": normalized["end_ms"],
        "files": loaded.files,
        "rows": loaded.rows,
        "first_time": loaded.first_ms,
        "last_time": loaded.last_ms,
        "runs": runs,
    }


def _write_json(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    handle, temp_name = tempfile.mkstemp(prefix=path.name, suffix=".tmp", dir=path.parent)
    try:
        with os.fdopen(handle, "w", encoding="utf-8") as file:
            json.dump(payload, file, ensure_ascii=False, allow_nan=False, separators=(",", ":"))
        os.replace(temp_name, path)
    finally:
        if os.path.exists(temp_name):
            os.unlink(temp_name)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="TradingView vectorbt backtest worker")
    parser.add_argument("--request", required=True, help="request JSON path")
    parser.add_argument("--output", required=True, help="result JSON path")
    parser.add_argument("--version", action="version", version=__version__)
    args = parser.parse_args(argv)

    output = Path(args.output)
    try:
        request = json.loads(Path(args.request).read_text(encoding="utf-8"))
        payload = execute(request)
        _write_json(output, payload)
        return 0
    except Exception as exc:
        traceback.print_exc()
        try:
            _write_json(
                output,
                {"error": f"{type(exc).__name__}: {exc}", "worker_version": WORKER_VERSION},
            )
        except Exception:
            pass
        return 1


if __name__ == "__main__":
    sys.exit(main())
