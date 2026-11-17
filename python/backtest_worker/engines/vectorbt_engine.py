"""vectorbt 1.2.0 适配器。

输入转换：`EngineRequest` -> `vbt.Portfolio.from_signals`。
输出转换：vbt 的 equity/drawdown/trades/returns -> Rust 契约字典。

撮合、交易记录、回撤与指标全部取 vectorbt 的结果；本文件只做字段改名、单位
换算（小数 -> 百分比）与曲线降采样。唯一的特化是 `_equity_from_orders`：
`portfolio.value()` 在 485 万根 1m bar 上实测 19–28 秒，整组会从 17 秒级升到
60 秒以上，AC-P01 的 30 秒上限失守。这里改为消费 `orders.records` 线性填充，
结果与 `value()` 逐 bar 完全相同（实测 max abs diff 0.0）。
"""

from __future__ import annotations

import math
from typing import Any

import numpy as np
import pandas as pd
import vectorbt as vbt

from ..curve import select_curve_indices
from ..engine import EngineError, EngineRequest
from ..timeframes import PERIODS_PER_YEAR

NAME = "vectorbt"

# pandas 3 把日频小写 `d` 标为弃用，vectorbt 解析 freq 时会告警。
_FREQ_ALIASES = {"1d": "1D"}


def _finite_or_none(value: Any) -> float | None:
    try:
        number = float(value)
    except (TypeError, ValueError):
        return None
    return number if math.isfinite(number) else None


def _times_ms(index: pd.DatetimeIndex) -> np.ndarray:
    return index.as_unit("ms").asi8.astype(np.int64)


def _equity_from_orders(
    orders: pd.DataFrame,
    close: np.ndarray,
    initial_balance: float,
) -> np.ndarray:
    """按订单记录重建逐 bar 权益。

    策略只做全仓进出：买单后现金归零，卖单后现金为成交额减手续费。未持仓区间
    现金保持上一笔卖出的余额，持仓区间按收盘价 mark-to-market。
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


class VectorBTEngine:
    name = NAME

    def version(self) -> str:
        return getattr(vbt, "__version__", "unknown")

    def run(self, request: EngineRequest) -> dict[str, object]:
        close = request.close_price.astype("float64")
        freq = _FREQ_ALIASES.get(request.timeframe, request.timeframe)
        portfolio = vbt.Portfolio.from_signals(
            close=close,
            entries=request.entries,
            exits=request.exits,
            price=request.open_price.astype("float64"),
            init_cash=float(request.initial_balance),
            fees=float(request.fee_bps) / 10_000.0,
            slippage=float(request.slippage_bps) / 10_000.0,
            direction="longonly",
            freq=freq,
        )

        equity_values = _equity_from_orders(
            portfolio.orders.records,
            close.to_numpy(dtype=np.float64),
            float(request.initial_balance),
        )
        if not np.isfinite(equity_values).all():
            raise EngineError("engine returned non-finite equity")
        equity = pd.Series(equity_values, index=close.index)

        times = _times_ms(close.index)
        # vbt 的 Series accessor 用 DatetimeIndex 包装 485 万点结果要 4 秒以上；
        # drawdown 是逐点运算、与索引无关，改用 RangeIndex 调同一个 accessor，
        # 再按位置取水下曲线。
        drawdown_series = pd.Series(equity_values).vbt.drawdown()
        underwater_pct = drawdown_series.to_numpy(dtype=np.float64) * 100.0
        drawdown_records = equity.vbt.drawdowns.records
        returns = equity.pct_change().fillna(0.0).vbt.returns(freq=freq)

        trades, total_trades = self._trades(
            portfolio.trades.records,
            times,
            int(request.max_trades),
        )
        metrics = self._metrics(
            portfolio=portfolio,
            equity=equity,
            returns=returns,
            drawdown_series=drawdown_series,
            underwater_pct=underwater_pct,
            drawdown_records=drawdown_records,
            times=times,
            timeframe=request.timeframe,
        )
        troughs = [
            (int(row.valley_idx), float((1.0 - row.valley_val / row.peak_val) * 100.0))
            for row in drawdown_records.itertuples(index=False)
        ]
        selected = select_curve_indices(
            len(equity_values),
            int(request.max_curve_points),
            troughs,
        )

        return {
            "metrics": metrics,
            "trades": trades,
            "trades_truncated": total_trades > len(trades),
            "drawdown_events": self._drawdown_events(drawdown_records, times),
            "equity": [[int(times[i]), float(equity_values[i])] for i in selected],
            "underwater": [[int(times[i]), float(underwater_pct[i])] for i in selected],
        }

    @staticmethod
    def _trades(
        records: pd.DataFrame,
        times: np.ndarray,
        max_trades: int,
    ) -> tuple[list[dict[str, object]], int]:
        total = int(len(records))
        trades: list[dict[str, object]] = []
        for position in range(min(total, max(0, max_trades))):
            row = records.iloc[position]
            entry_idx = int(row["entry_idx"])
            exit_idx = int(row["exit_idx"])
            trade_return = _finite_or_none(row["return"])
            trades.append(
                {
                    "seq": position,
                    "entry_time": int(times[entry_idx]),
                    "exit_time": int(times[exit_idx]),
                    "side": "long",
                    # vectorbt TradeStatus: 0=Open, 1=Closed。
                    "status": "closed" if int(row["status"]) == 1 else "open",
                    "entry_price": _finite_or_none(row["entry_price"]),
                    "exit_price": _finite_or_none(row["exit_price"]),
                    "qty": _finite_or_none(row["size"]),
                    "pnl": _finite_or_none(row["pnl"]),
                    "pnl_pct": (trade_return * 100.0) if trade_return is not None else None,
                    "bars_held": max(0, exit_idx - entry_idx),
                }
            )
        return trades, total

    @staticmethod
    def _metrics(
        *,
        portfolio: Any,
        equity: pd.Series,
        returns: Any,
        drawdown_series: pd.Series,
        underwater_pct: np.ndarray,
        drawdown_records: pd.DataFrame,
        times: np.ndarray,
        timeframe: str,
    ) -> dict[str, object]:
        trades = portfolio.trades
        total_trades = int(trades.count())
        bars = int(len(equity))

        if total_trades:
            win_rate = _finite_or_none(trades.win_rate() * 100.0)
            profit_factor = _finite_or_none(trades.profit_factor())
            expectancy = _finite_or_none(trades.returns.mean())
        else:
            win_rate = None
            profit_factor = None
            expectancy = None

        if len(drawdown_records):
            depths = (1.0 - drawdown_records["valley_val"] / drawdown_records["peak_val"]) * 100.0
            avg_drawdown_pct = _finite_or_none(depths.mean())
            status = drawdown_records["status"].to_numpy()
            peak_idx = drawdown_records["peak_idx"].to_numpy()
            valley_idx = drawdown_records["valley_idx"].to_numpy()
            end_idx = drawdown_records["end_idx"].to_numpy()
            recovered = status == 1
            duration_end = np.where(recovered, end_idx, bars - 1)
            longest_drawdown_ms = int((times[duration_end] - times[peak_idx]).max())
            longest_recovery_ms = (
                int((times[end_idx[recovered]] - times[valley_idx[recovered]]).max())
                if recovered.any()
                else 0
            )
        else:
            avg_drawdown_pct = 0.0
            longest_drawdown_ms = 0
            longest_recovery_ms = 0

        max_drawdown_pct = float(max(0.0, -float(drawdown_series.min()) * 100.0))

        return {
            "num_trades": total_trades,
            "win_rate": win_rate,
            "total_return_pct": _finite_or_none(returns.total() * 100.0),
            "sharpe_ratio": _finite_or_none(returns.sharpe_ratio()),
            "sortino_ratio": _finite_or_none(returns.sortino_ratio()),
            "max_drawdown_pct": _finite_or_none(max_drawdown_pct),
            "avg_drawdown_pct": avg_drawdown_pct,
            "drawdown_count": int(len(drawdown_records)),
            "longest_drawdown_days": _finite_or_none(longest_drawdown_ms / 86_400_000.0),
            "longest_recovery_days": _finite_or_none(longest_recovery_ms / 86_400_000.0),
            "drawdown_time_pct": _finite_or_none(float((underwater_pct < 0.0).mean() * 100.0)),
            "profit_factor": profit_factor,
            "expectancy": expectancy,
            "final_equity": _finite_or_none(equity.iloc[-1]),
            "bars": bars,
            "periods_per_year": float(PERIODS_PER_YEAR[timeframe]),
        }

    @staticmethod
    def _drawdown_events(
        records: pd.DataFrame,
        times: np.ndarray,
    ) -> list[dict[str, object]]:
        events: list[dict[str, object]] = []
        last_time = int(times[-1])
        for seq, row in enumerate(records.itertuples(index=False)):
            recovered = int(row.status) == 1
            peak_index = int(row.peak_idx)
            trough_index = int(row.valley_idx)
            recovery_time = int(times[int(row.end_idx)]) if recovered else None
            duration_end = recovery_time if recovery_time is not None else last_time
            events.append(
                {
                    "seq": seq,
                    "peak_time": int(times[peak_index]),
                    "trough_time": int(times[trough_index]),
                    "recovery_time": recovery_time,
                    "depth_pct": float((1.0 - row.valley_val / row.peak_val) * 100.0),
                    "duration_ms": int(duration_end - int(times[peak_index])),
                    "recovery_ms": (
                        int(recovery_time - times[trough_index])
                        if recovery_time is not None
                        else None
                    ),
                    "recovered": recovered,
                }
            )
        return events
