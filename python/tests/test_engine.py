from __future__ import annotations

import unittest

import pandas as pd

try:
    import vectorbt  # noqa: F401

    HAS_VECTORBT = True
except ImportError:
    HAS_VECTORBT = False

from backtest_worker.engine import EngineRequest, get_engine
from backtest_worker.strategy import dual_ma_signals


def make_frame(closes: list[float]) -> pd.DataFrame:
    index = pd.date_range("2024-01-01", periods=len(closes), freq="D", tz="UTC")
    return pd.DataFrame(
        {
            "open": [value + 0.5 for value in closes],
            "high": [value + 2 for value in closes],
            "low": [value - 2 for value in closes],
            "close": closes,
            "volume": [10.0] * len(closes),
        },
        index=index,
    )


def ms(index: pd.DatetimeIndex, position: int) -> int:
    return int(index[position].value // 1_000_000)


@unittest.skipUnless(HAS_VECTORBT, "vectorbt not installed")
class EngineTest(unittest.TestCase):
    def run_engine(
        self,
        frame: pd.DataFrame,
        fast: int = 2,
        slow: int = 4,
        max_curve_points: int = 100,
        max_trades: int = 1000,
    ) -> dict[str, object]:
        entries, exits = dual_ma_signals(frame, fast, slow)
        return get_engine().run(
            EngineRequest(
                timeframe="1d",
                close_price=frame["close"],
                open_price=frame["open"],
                entries=entries,
                exits=exits,
                fee_bps=10.0,
                slippage_bps=5.0,
                initial_balance=10_000.0,
                max_curve_points=max_curve_points,
                max_trades=max_trades,
            )
        )

    def frame(self) -> pd.DataFrame:
        closes = [100, 101, 102, 103, 104, 103, 102, 101, 100, 101, 103, 105, 107]
        return make_frame(closes)

    def test_trades_and_determinism(self) -> None:
        frame = self.frame()
        first = self.run_engine(frame)
        second = self.run_engine(frame)
        self.assertEqual(first["metrics"], second["metrics"])
        self.assertEqual(first["trades"], second["trades"])
        self.assertGreaterEqual(first["metrics"]["num_trades"], 1)  # type: ignore[index]
        self.assertLessEqual(len(first["equity"]), 100)  # type: ignore[arg-type]
        self.assertEqual(first["equity"][-1][0], ms(frame.index, len(frame) - 1))  # type: ignore[index]

    def test_no_lookahead_first_entry_after_slow_window(self) -> None:
        closes = [100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110]
        frame = make_frame(closes)
        result = self.run_engine(frame, fast=2, slow=4)
        trades = result["trades"]  # type: ignore[index]
        self.assertTrue(trades)
        first_entry = trades[0]["entry_time"]
        slow_ready_time = ms(frame.index, 3)
        self.assertGreaterEqual(first_entry, slow_ready_time)

    def test_insufficient_bars_raises(self) -> None:
        frame = make_frame([100, 101, 102])
        with self.assertRaises(Exception):
            self.run_engine(frame, fast=2, slow=10)

    def test_metrics_contract(self) -> None:
        result = self.run_engine(self.frame())
        metrics = result["metrics"]
        self.assertEqual(
            set(metrics),  # type: ignore[arg-type]
            {
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
                "bars",
                "periods_per_year",
            },
        )
        self.assertEqual(metrics["bars"], 13)  # type: ignore[index]
        self.assertEqual(metrics["periods_per_year"], 365.0)  # type: ignore[index]
        self.assertGreater(metrics["max_drawdown_pct"], 0.0)  # type: ignore[index]

    def test_metrics_gold(self) -> None:
        metrics = self.run_engine(self.frame())["metrics"]
        # 冻结 vectorbt 1.2.0 在固定序列上的输出，防止适配器映射或口径漂移。
        self.assertEqual(metrics["num_trades"], 2)  # type: ignore[index]
        self.assertEqual(metrics["drawdown_count"], 1)  # type: ignore[index]
        self.assertAlmostEqual(metrics["final_equity"], 9806.793163547278, places=6)  # type: ignore[index]
        self.assertAlmostEqual(metrics["total_return_pct"], -1.9320683645272174, places=6)  # type: ignore[index]
        self.assertAlmostEqual(metrics["max_drawdown_pct"], 3.7651138156575503, places=6)  # type: ignore[index]
        self.assertAlmostEqual(metrics["sharpe_ratio"], -3.8580735440579303, places=6)  # type: ignore[index]
        self.assertAlmostEqual(metrics["sortino_ratio"], -5.810732360737825, places=6)  # type: ignore[index]

    def test_drawdown_event_contract(self) -> None:
        frame = self.frame()
        result = self.run_engine(frame)
        events = result["drawdown_events"]  # type: ignore[index]
        self.assertTrue(events)
        last_time = ms(frame.index, len(frame) - 1)
        for seq, event in enumerate(events):
            self.assertEqual(event["seq"], seq)
            self.assertGreaterEqual(event["depth_pct"], 0.0)
            if event["recovered"]:
                self.assertIsNotNone(event["recovery_time"])
                self.assertEqual(
                    event["recovery_ms"],
                    event["recovery_time"] - event["trough_time"],
                )
                self.assertEqual(
                    event["duration_ms"],
                    event["recovery_time"] - event["peak_time"],
                )
            else:
                self.assertIsNone(event["recovery_time"])
                self.assertIsNone(event["recovery_ms"])
                self.assertEqual(event["duration_ms"], last_time - event["peak_time"])

    def test_curves_and_truncation(self) -> None:
        frame = self.frame()
        result = self.run_engine(frame, max_curve_points=10, max_trades=1)
        equity = result["equity"]  # type: ignore[index]
        underwater = result["underwater"]  # type: ignore[index]
        self.assertLessEqual(len(equity), 10)
        self.assertEqual([point[0] for point in equity], [point[0] for point in underwater])
        self.assertEqual(equity[-1][0], ms(frame.index, len(frame) - 1))
        self.assertEqual(len(result["trades"]), 1)  # type: ignore[arg-type]
        if result["metrics"]["num_trades"] > 1:  # type: ignore[index]
            self.assertTrue(result["trades_truncated"])


if __name__ == "__main__":
    unittest.main()
