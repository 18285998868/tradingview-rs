"""回测指标。

所有公式在本文件内实现并测试，不依赖 vectorbt 的统计口径。年化系数按
7×24 交易、UTC、无交易日历计算。
"""

from __future__ import annotations

from typing import Iterable, Sequence

import numpy as np

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


def _finite_or_none(value: float | None) -> float | None:
    if value is None:
        return None
    value = float(value)
    return value if np.isfinite(value) else None


def compute_metrics(
    *,
    timeframe: str,
    equity: Sequence[float],
    initial_balance: float,
    trade_pnls: Iterable[float],
    trade_returns: Iterable[float],
    max_drawdown_pct: float,
    avg_drawdown_pct: float,
    drawdown_count: int,
    longest_drawdown_ms: int,
    longest_recovery_ms: int,
    drawdown_time_pct: float,
) -> dict[str, object]:
    """由权益与交易收益计算汇总指标。"""

    values = np.asarray(equity, dtype=np.float64)
    if values.size == 0:
        raise ValueError("equity must not be empty")
    final_equity = float(values[-1])
    total_return_pct = (final_equity / initial_balance - 1.0) * 100.0

    pnls = np.asarray(list(trade_pnls), dtype=np.float64)
    returns = np.asarray(list(trade_returns), dtype=np.float64)
    num_trades = int(pnls.size)

    win_rate: float | None = None
    profit_factor: float | None = None
    expectancy: float | None = None
    if num_trades:
        wins = pnls > 0
        win_rate = float(wins.mean() * 100.0)
        gross_profit = float(pnls[wins].sum())
        gross_loss = float(-pnls[~wins].sum())
        if gross_loss > 0:
            profit_factor = gross_profit / gross_loss
        elif gross_profit > 0:
            profit_factor = None
        else:
            profit_factor = None
        finite_returns = returns[np.isfinite(returns)]
        if finite_returns.size:
            expectancy = float(finite_returns.mean())

    sharpe_ratio: float | None = None
    sortino_ratio: float | None = None
    if values.size >= 3:
        bar_returns = np.diff(values) / values[:-1]
        bar_returns = bar_returns[np.isfinite(bar_returns)]
        if bar_returns.size >= 2:
            std = float(bar_returns.std(ddof=1))
            mean = float(bar_returns.mean())
            if std > 0:
                sharpe_ratio = mean / std * np.sqrt(PERIODS_PER_YEAR[timeframe])
            downside = bar_returns[bar_returns < 0]
            if downside.size:
                downside_dev = float(np.sqrt(np.mean(downside**2)))
                if downside_dev > 0:
                    sortino_ratio = mean / downside_dev * np.sqrt(PERIODS_PER_YEAR[timeframe])

    return {
        "num_trades": num_trades,
        "win_rate": _finite_or_none(win_rate),
        "total_return_pct": _finite_or_none(total_return_pct),
        "sharpe_ratio": _finite_or_none(sharpe_ratio),
        "sortino_ratio": _finite_or_none(sortino_ratio),
        "max_drawdown_pct": _finite_or_none(max_drawdown_pct),
        "avg_drawdown_pct": _finite_or_none(avg_drawdown_pct),
        "drawdown_count": int(drawdown_count),
        "longest_drawdown_days": _finite_or_none(longest_drawdown_ms / 86_400_000.0),
        "longest_recovery_days": _finite_or_none(longest_recovery_ms / 86_400_000.0),
        "drawdown_time_pct": _finite_or_none(drawdown_time_pct),
        "profit_factor": _finite_or_none(profit_factor),
        "expectancy": _finite_or_none(expectancy),
        "final_equity": _finite_or_none(final_equity),
        "bars": int(values.size),
        "periods_per_year": float(PERIODS_PER_YEAR[timeframe]),
    }
