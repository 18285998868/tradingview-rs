"""与 vectorbt 无关的双均线参考实现。

只用于对账测试：同样的数据、参数与成本口径，用显式现金/持仓循环重算一遍，
再与 `backtest_worker.engine.run_dual_ma` 的输出比较。这里刻意不复用生产代码的
指标、回撤与撮合函数，避免同一个错误被两边同时复制。

撮合口径（与 vectorbt `Portfolio.from_orders(size_type="targetpercent")` 对齐）：

- 目标仓位由 T 收盘均线比较后右移一根 bar；
- 目标 0 -> 1 时按 `open * (1 + slippage)` 全仓买入，订单金额为 `cash / (1 + fee)`；
- 目标 1 -> 0 时按 `open * (1 - slippage)` 全部卖出，手续费从成交额扣除；
- 持仓不变时不调仓；
- 未平仓交易按最后一根收盘价标记，不计卖出滑点与卖出手续费。
"""

from __future__ import annotations

import math
from dataclasses import dataclass
from typing import Any, Sequence

import numpy as np
import pandas as pd

PERIODS_PER_YEAR: dict[str, float] = {
    "1m": 365 * 24 * 60,
    "3m": 365 * 24 * 20,
    "5m": 365 * 24 * 12,
    "15m": 365 * 24 * 4,
    "30m": 365 * 24 * 2,
    "1h": 365 * 24,
    "2h": 365 * 12,
    "4h": 365 * 6,
    "6h": 365 * 4,
    "8h": 365 * 3,
    "12h": 365 * 2,
    "1d": 365,
}


@dataclass
class ReferenceTrade:
    """一笔参考交易。"""

    entry_index: int
    exit_index: int
    entry_price: float
    exit_price: float
    qty: float
    entry_fee: float
    exit_fee: float
    pnl: float
    trade_return: float
    status: str


@dataclass
class ReferenceDrawdown:
    """参考回撤结果。"""

    events: list[dict[str, Any]]
    underwater_pct: np.ndarray
    drawdown_time_pct: float
    max_drawdown_pct: float
    avg_drawdown_pct: float
    longest_drawdown_ms: int
    longest_recovery_ms: int


def dual_ma_target(close: pd.Series, fast: int, slow: int) -> np.ndarray:
    """T 收盘信号右移一根 bar，得到每根 bar 的目标仓位。"""

    fast_ma = close.rolling(fast, min_periods=fast).mean()
    slow_ma = close.rolling(slow, min_periods=slow).mean()
    target = (fast_ma > slow_ma).astype("float64").shift(1).fillna(0.0)
    return target.to_numpy(dtype=np.float64)


def compute_drawdown_reference(times_ms: Sequence[int], equity: Sequence[float]) -> ReferenceDrawdown:
    """显式循环识别回撤事件，独立于生产实现的向量化写法。"""

    times = [int(value) for value in times_ms]
    values = [float(value) for value in equity]
    if len(times) != len(values) or not values:
        raise ValueError("times and equity must be non-empty and equal length")

    events: list[dict[str, Any]] = []
    underwater: list[float] = []
    peak_value = values[0]
    peak_index = 0
    in_drawdown_bars = 0

    index = 0
    while index < len(values):
        value = values[index]
        if value >= peak_value:
            peak_value = value
            peak_index = index
            underwater.append(0.0)
            index += 1
            continue

        start = index
        trough_index = index
        while index < len(values) and values[index] < peak_value:
            if values[index] < values[trough_index]:
                trough_index = index
            underwater.append((values[index] / peak_value - 1.0) * 100.0)
            in_drawdown_bars += 1
            index += 1
        end = index - 1
        recovered = index < len(values)
        recovery_index = index if recovered else None
        recovery_time = times[recovery_index] if recovery_index is not None else None
        last_time = times[end]
        depth_pct = (peak_value - values[trough_index]) / peak_value * 100.0
        events.append(
            {
                "peak_time": times[peak_index],
                "trough_time": times[trough_index],
                "recovery_time": recovery_time,
                "depth_pct": depth_pct,
                "duration_ms": (recovery_time if recovery_time is not None else last_time)
                - times[peak_index],
                "recovery_ms": (
                    recovery_time - times[trough_index] if recovery_time is not None else None
                ),
                "recovered": recovered,
                "peak_index": peak_index,
                "trough_index": trough_index,
            }
        )
        # 恢复 bar 由下一轮外层循环处理：它会刷新峰值并补一个 0 点。

    max_depth = max((event["depth_pct"] for event in events), default=0.0)
    avg_depth = sum(event["depth_pct"] for event in events) / len(events) if events else 0.0
    longest_duration = max((event["duration_ms"] for event in events), default=0)
    longest_recovery = max(
        (event["recovery_ms"] or 0 for event in events if event["recovered"]), default=0
    )
    return ReferenceDrawdown(
        events=events,
        underwater_pct=np.asarray(underwater, dtype=np.float64),
        drawdown_time_pct=in_drawdown_bars / len(values) * 100.0,
        max_drawdown_pct=max_depth,
        avg_drawdown_pct=avg_depth,
        longest_drawdown_ms=longest_duration,
        longest_recovery_ms=longest_recovery,
    )


def run_dual_ma_reference(
    *,
    frame: pd.DataFrame,
    params: dict[str, int],
    timeframe: str,
    fee_bps: float,
    slippage_bps: float,
    initial_balance: float,
) -> dict[str, Any]:
    """显式现金/持仓循环，返回与 worker 可比较的权益、交易与指标。"""

    fast = int(params["fast"])
    slow = int(params["slow"])
    if fast < 1 or slow < 2 or fast >= slow:
        raise ValueError(f"invalid dual_ma params: fast={fast}, slow={slow}")
    if len(frame) < slow + 2:
        raise ValueError(f"insufficient bars for fast={fast}, slow={slow}")

    opens = frame["open"].astype("float64").to_numpy()
    closes = frame["close"].astype("float64").to_numpy()
    times = [int(round(stamp.timestamp() * 1000.0)) for stamp in frame.index]
    target = dual_ma_target(frame["close"].astype("float64"), fast, slow)

    fee = float(fee_bps) / 10_000.0
    slippage = float(slippage_bps) / 10_000.0
    cash = float(initial_balance)
    qty = 0.0
    entry: dict[str, Any] | None = None
    trades: list[ReferenceTrade] = []
    equity: list[float] = []

    for index, target_value in enumerate(target):
        if target_value > 0.0 and qty == 0.0:
            order_value = cash / (1.0 + fee)
            price = float(opens[index]) * (1.0 + slippage)
            qty = order_value / price
            entry_fee = order_value * fee
            cash = 0.0
            entry = {
                "index": index,
                "price": price,
                "fee": entry_fee,
                "order_value": order_value,
            }
        elif target_value <= 0.0 and qty > 0.0:
            assert entry is not None
            price = float(opens[index]) * (1.0 - slippage)
            gross = qty * price
            exit_fee = gross * fee
            cash = gross - exit_fee
            pnl = qty * (price - entry["price"]) - entry["fee"] - exit_fee
            trades.append(
                ReferenceTrade(
                    entry_index=entry["index"],
                    exit_index=index,
                    entry_price=entry["price"],
                    exit_price=price,
                    qty=qty,
                    entry_fee=entry["fee"],
                    exit_fee=exit_fee,
                    pnl=pnl,
                    trade_return=pnl / entry["order_value"],
                    status="closed",
                )
            )
            qty = 0.0
            entry = None
        equity.append(cash + qty * float(closes[index]))

    if qty > 0.0:
        assert entry is not None
        last = len(closes) - 1
        price = float(closes[last])
        pnl = qty * (price - entry["price"]) - entry["fee"]
        trades.append(
            ReferenceTrade(
                entry_index=entry["index"],
                exit_index=last,
                entry_price=entry["price"],
                exit_price=price,
                qty=qty,
                entry_fee=entry["fee"],
                exit_fee=0.0,
                pnl=pnl,
                trade_return=pnl / entry["order_value"],
                status="open",
            )
        )

    drawdown = compute_drawdown_reference(times, equity)
    trade_pnls = [trade.pnl for trade in trades]
    trade_returns = [trade.trade_return for trade in trades]
    metrics = _compute_metrics_reference(
        timeframe=timeframe,
        equity=np.asarray(equity, dtype=np.float64),
        initial_balance=float(initial_balance),
        trade_pnls=trade_pnls,
        trade_returns=trade_returns,
        drawdown=drawdown,
    )
    return {
        "equity": np.asarray(equity, dtype=np.float64),
        "times": times,
        "target": target,
        "trades": trades,
        "drawdown": drawdown,
        "metrics": metrics,
    }


def _compute_metrics_reference(
    *,
    timeframe: str,
    equity: np.ndarray,
    initial_balance: float,
    trade_pnls: list[float],
    trade_returns: list[float],
    drawdown: ReferenceDrawdown,
) -> dict[str, Any]:
    """独立实现的汇总指标。"""

    final_equity = float(equity[-1])
    total_return_pct = (final_equity / initial_balance - 1.0) * 100.0
    pnls = np.asarray(trade_pnls, dtype=np.float64)
    returns = np.asarray(trade_returns, dtype=np.float64)
    num_trades = int(pnls.size)

    win_rate = None
    profit_factor = None
    expectancy = None
    if num_trades:
        wins = pnls > 0
        win_rate = float(wins.mean() * 100.0)
        gross_profit = float(pnls[wins].sum())
        gross_loss = float(-pnls[~wins].sum())
        profit_factor = gross_profit / gross_loss if gross_loss > 0 else None
        expectancy = float(returns.mean()) if returns.size else None

    sharpe = None
    sortino = None
    if equity.size >= 3:
        bar_returns = np.diff(equity) / equity[:-1]
        bar_returns = bar_returns[np.isfinite(bar_returns)]
        if bar_returns.size >= 2:
            std = float(bar_returns.std(ddof=1))
            mean = float(bar_returns.mean())
            if std > 0:
                sharpe = mean / std * math.sqrt(PERIODS_PER_YEAR[timeframe])
            downside = bar_returns[bar_returns < 0]
            if downside.size:
                downside_dev = float(np.sqrt(np.mean(downside**2)))
                if downside_dev > 0:
                    sortino = mean / downside_dev * math.sqrt(PERIODS_PER_YEAR[timeframe])

    return {
        "num_trades": num_trades,
        "win_rate": win_rate,
        "total_return_pct": total_return_pct,
        "sharpe_ratio": sharpe,
        "sortino_ratio": sortino,
        "max_drawdown_pct": drawdown.max_drawdown_pct,
        "avg_drawdown_pct": drawdown.avg_drawdown_pct,
        "drawdown_count": len(drawdown.events),
        "longest_drawdown_days": drawdown.longest_drawdown_ms / 86_400_000.0,
        "longest_recovery_days": drawdown.longest_recovery_ms / 86_400_000.0,
        "drawdown_time_pct": drawdown.drawdown_time_pct,
        "profit_factor": profit_factor,
        "expectancy": expectancy,
        "final_equity": final_equity,
    }
