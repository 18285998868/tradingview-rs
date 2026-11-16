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


_WORKER_FRAME: Any = None
_WORKER_SETTINGS: dict[str, Any] | None = None


def _worker_count(count: int) -> int:
    """参数扫描的进程数；默认最多 4 个，可用环境变量覆盖。"""

    if count <= 1:
        return 1
    raw = os.environ.get("TRADINGVIEW_WORKERS")
    if raw is not None:
        try:
            value = int(raw)
        except ValueError as exc:
            raise ValueError(f"TRADINGVIEW_WORKERS must be an integer, got {raw!r}") from exc
        return max(1, min(value, count))
    return max(1, min(4, os.cpu_count() or 1, count))


def _run_one(
    frame: Any,
    params: dict[str, int],
    normalized: dict[str, Any],
) -> dict[str, Any]:
    print(
        f"running {normalized['pair']} {normalized['timeframe']} "
        f"fast={params['fast']} slow={params['slow']} bars={len(frame)}",
        flush=True,
    )
    try:
        return run_dual_ma(
            frame=frame,
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
        return {
            "params": params,
            "params_text": f"fast={params['fast']},slow={params['slow']}",
            "bars": len(frame),
            "first_time": int(frame.index[0].timestamp() * 1000),
            "last_time": int(frame.index[-1].timestamp() * 1000),
            "metrics": None,
            "trades": [],
            "trades_truncated": False,
            "drawdown_events": [],
            "equity": [],
            "underwater": [],
            "error": f"{type(exc).__name__}: {exc}",
        }


def _worker_run(params: dict[str, int]) -> dict[str, Any]:
    if _WORKER_FRAME is None or _WORKER_SETTINGS is None:
        raise RuntimeError("worker frame is not initialized")
    return _run_one(_WORKER_FRAME, params, _WORKER_SETTINGS)


def _run_params(frame: Any, normalized: dict[str, Any]) -> list[dict[str, Any]]:
    """执行一批参数；多组时用 fork 进程并行，保持参数顺序。"""

    params_list: list[dict[str, int]] = normalized["params_list"]
    workers = _worker_count(len(params_list))
    if workers <= 1:
        return [_run_one(frame, params, normalized) for params in params_list]

    import multiprocessing

    try:
        context = multiprocessing.get_context("fork")
    except ValueError:
        # 平台没有 fork 时退回串行，避免把整个行情表 pickle 到子进程。
        return [_run_one(frame, params, normalized) for params in params_list]

    global _WORKER_FRAME, _WORKER_SETTINGS
    _WORKER_FRAME = frame
    _WORKER_SETTINGS = normalized
    try:
        with context.Pool(processes=workers) as pool:
            return pool.map(_worker_run, params_list, chunksize=1)
    finally:
        _WORKER_FRAME = None
        _WORKER_SETTINGS = None


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
    runs = _run_params(loaded.frame, normalized)

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
