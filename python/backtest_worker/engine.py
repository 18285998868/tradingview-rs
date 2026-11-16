"""双均线只做多引擎（vectorbt 1.2.0）。

信号口径：T 收盘比较 fast/slow 均线，目标仓位在 T+1 开盘执行。实现上把目标
仓位右移一根 bar，成交价用当根开盘价，权益按当根收盘价 mark-to-market。
vectorbt 的 `from_orders` 只有一个价格序列，无法区分成交价与估值价，因此改用
`from_signals(close=收盘价, price=开盘价)`；未平仓交易也按最后一根收盘价标记。
"""

from __future__ import annotations

import math
from typing import Any, Sequence

import numpy as np
import pandas as pd

from .drawdown import DrawdownEvent, compute_drawdown
from .metrics import compute_metrics

STRATEGY = "dual_ma"
STRATEGY_VERSION = "1"
WORKER_VERSION = "0.1.0"


class EngineError(RuntimeError):
    """策略无法执行（数据不足、依赖缺失等）。"""


def _safe_float(value: Any) -> float | None:
    try:
        number = float(value)
    except (TypeError, ValueError):
        return None
    return number if math.isfinite(number) else None


def _timestamp_ms(index: pd.DatetimeIndex, position: int) -> int:
    stamp = index[int(position)]
    return int(round(stamp.timestamp() * 1000.0))


def _equity_from_orders(
    orders: pd.DataFrame,
    close: np.ndarray,
    initial_balance: float,
) -> np.ndarray:
    """按订单记录重建逐 bar 权益。

    vectorbt 的 `value()` / `assets()` / `cash()` 在 485 万根 1m bar 上会重复
    模拟并花掉数十秒；这里直接消费订单记录，一次线性填充。策略只做全仓进出，
    买单后现金归零，卖单后现金为成交额减手续费。
    """

    count = close.size
    cash = np.empty(count, dtype=np.float64)
    qty = np.empty(count, dtype=np.float64)
    current_cash = float(initial_balance)
    current_qty = 0.0
    cursor = 0
    for row in orders.itertuples(index=False):
        index = int(row.idx)
        if index > cursor:
            cash[cursor:index] = current_cash
            qty[cursor:index] = current_qty
        if int(row.side) == 0:
            current_qty += float(row.size)
            current_cash = 0.0
        else:
            current_qty = 0.0
            current_cash = float(row.size) * float(row.price) - float(row.fees)
        cursor = index
    cash[cursor:] = current_cash
    qty[cursor:] = current_qty
    return cash + qty * close


def _select_curve_indices(
    size: int,
    max_points: int,
    events: Sequence[DrawdownEvent],
) -> list[int]:
    """选出降采样下标：均匀点 + 首尾 + 最深若干事件的谷底。"""

    if size <= max_points:
        return list(range(size))
    max_points = max(10, int(max_points))
    mandatory = {0, size - 1}
    top_events = sorted(events, key=lambda event: event.depth_pct, reverse=True)
    limit = max(5, max_points // 10)
    mandatory.update(event.trough_index for event in top_events[:limit])
    mandatory = {index for index in mandatory if 0 <= index < size}

    stride = max(1, math.ceil(size / max_points))
    selected = set(range(0, size, stride)) | mandatory
    if len(selected) > max_points:
        removable = sorted(selected - mandatory)
        keep = max(0, max_points - len(mandatory))
        if keep == 0:
            selected = set(sorted(mandatory)[:max_points])
        else:
            step = len(removable) / keep
            picked = {removable[min(len(removable) - 1, int(i * step))] for i in range(keep)}
            selected = mandatory | picked
    return sorted(selected)


def run_dual_ma(
    *,
    frame: pd.DataFrame,
    params: dict[str, int],
    timeframe: str,
    fee_bps: float,
    slippage_bps: float,
    initial_balance: float,
    max_curve_points: int,
    max_trades: int,
) -> dict[str, object]:
    """执行单组参数，返回与 Rust 契约一致的结果字典。"""

    try:
        import vectorbt as vbt
    except ImportError as exc:  # pragma: no cover - 部署环境缺失时给出可读错误
        raise EngineError("vectorbt is not installed in the selected Python environment") from exc

    fast = int(params["fast"])
    slow = int(params["slow"])
    if fast < 1 or slow < 2 or fast >= slow:
        raise EngineError(f"invalid dual_ma params: fast={fast}, slow={slow}")
    if len(frame) < slow + 2:
        raise EngineError(
            f"insufficient bars for fast={fast}, slow={slow}: have {len(frame)}, need {slow + 2}"
        )

    close = frame["close"].astype("float64")
    fast_ma = close.rolling(fast, min_periods=fast).mean()
    slow_ma = close.rolling(slow, min_periods=slow).mean()
    target = (fast_ma > slow_ma).astype("float64").shift(1).fillna(0.0)
    long_target = target > 0.0
    previous_target = long_target.shift(1, fill_value=False)
    entries = long_target & ~previous_target
    exits = ~long_target & previous_target

    portfolio = vbt.Portfolio.from_signals(
        close=close,
        entries=entries,
        exits=exits,
        price=frame["open"].astype("float64"),
        init_cash=float(initial_balance),
        fees=float(fee_bps) / 10_000.0,
        slippage=float(slippage_bps) / 10_000.0,
        direction="longonly",
    )

    close_values = close.to_numpy(dtype=np.float64)
    equity_values = _equity_from_orders(
        portfolio.orders.records,
        close_values,
        float(initial_balance),
    )
    if not np.isfinite(equity_values).all():
        raise EngineError("engine returned non-finite equity")

    times = frame.index.as_unit("ms").asi8.astype(np.int64)
    drawdown = compute_drawdown(times, equity_values)

    raw = portfolio.trades.records
    trades: list[dict[str, object]] = []
    trade_pnls: list[float] = []
    trade_returns: list[float] = []
    total_trades = int(len(raw))
    for position in range(min(total_trades, max(0, int(max_trades)))):
        row = raw.iloc[position]
        entry_idx = int(row["entry_idx"])
        exit_idx = int(row["exit_idx"])
        # vectorbt TradeStatus: 0=Open, 1=Closed。避免用 records_readable，
        # 它在百万级时间索引上会重复对齐并花掉十秒。
        status = "closed" if int(row["status"]) == 1 else "open"
        entry_time = _timestamp_ms(frame.index, entry_idx)
        exit_time = _timestamp_ms(frame.index, exit_idx)
        pnl = _safe_float(row["pnl"])
        trade_return = _safe_float(row["return"])
        if pnl is not None:
            trade_pnls.append(pnl)
        if trade_return is not None:
            trade_returns.append(trade_return)
        trades.append(
            {
                "seq": position,
                "entry_time": entry_time,
                "exit_time": exit_time,
                "side": "long",
                "status": status,
                "entry_price": _safe_float(row["entry_price"]),
                "exit_price": _safe_float(row["exit_price"]),
                "qty": _safe_float(row["size"]),
                "pnl": pnl,
                "pnl_pct": (trade_return * 100.0) if trade_return is not None else None,
                "bars_held": max(0, exit_idx - entry_idx),
            }
        )

    metrics = compute_metrics(
        timeframe=timeframe,
        equity=equity_values,
        initial_balance=float(initial_balance),
        trade_pnls=trade_pnls,
        trade_returns=trade_returns,
        max_drawdown_pct=drawdown.max_drawdown_pct,
        avg_drawdown_pct=drawdown.avg_drawdown_pct,
        drawdown_count=len(drawdown.events),
        longest_drawdown_ms=drawdown.longest_drawdown_ms,
        longest_recovery_ms=drawdown.longest_recovery_ms,
        drawdown_time_pct=drawdown.drawdown_time_pct,
    )
    # 交易被截断时，指标仍按实际全部交易计算，前端会显示 trades_truncated。
    if total_trades > len(trades):
        metrics["num_trades"] = total_trades

    max_points = max(10, int(max_curve_points))
    selected = _select_curve_indices(len(equity_values), max_points, drawdown.events)
    equity_points = [[int(times[i]), float(equity_values[i])] for i in selected]
    underwater_points = [
        [int(times[i]), float(drawdown.underwater_pct[i])] for i in selected
    ]

    return {
        "params": {"fast": fast, "slow": slow},
        "params_text": f"fast={fast},slow={slow}",
        "bars": int(len(frame)),
        "first_time": int(times[0]),
        "last_time": int(times[-1]),
        "metrics": metrics,
        "trades": trades,
        "trades_truncated": bool(total_trades > len(trades)),
        "drawdown_events": [
            event.to_dict(seq) for seq, event in enumerate(drawdown.events)
        ],
        "equity": equity_points,
        "underwater": underwater_points,
        "error": None,
    }
