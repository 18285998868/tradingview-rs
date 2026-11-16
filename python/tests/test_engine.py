from __future__ import annotations

import unittest
from pathlib import Path

import pandas as pd

try:
    import vectorbt  # noqa: F401

    HAS_VECTORBT = True
except ImportError:
    HAS_VECTORBT = False

from backtest_worker.engine import run_dual_ma


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


@unittest.skipUnless(HAS_VECTORBT, "vectorbt not installed")
class EngineTest(unittest.TestCase):
    def run_engine(self, frame: pd.DataFrame, fast: int = 2, slow: int = 4) -> dict[str, object]:
        return run_dual_ma(
            frame=frame,
            params={"fast": fast, "slow": slow},
            timeframe="1d",
            fee_bps=10.0,
            slippage_bps=5.0,
            initial_balance=10_000.0,
            max_curve_points=100,
            max_trades=1000,
        )

    def test_trades_and_determinism(self) -> None:
        closes = [100, 101, 102, 103, 104, 103, 102, 101, 100, 101, 103, 105, 107]
        frame = make_frame(closes)
        first = self.run_engine(frame)
        second = self.run_engine(frame)
        self.assertEqual(first["metrics"], second["metrics"])
        self.assertEqual(first["trades"], second["trades"])
        self.assertGreaterEqual(first["metrics"]["num_trades"], 1)  # type: ignore[index]
        self.assertLessEqual(len(first["equity"]), 100)  # type: ignore[arg-type]
        self.assertEqual(first["equity"][-1][0], first["last_time"])  # type: ignore[index]

    def test_no_lookahead_first_entry_after_slow_window(self) -> None:
        closes = [100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110]
        frame = make_frame(closes)
        result = self.run_engine(frame, fast=2, slow=4)
        trades = result["trades"]  # type: ignore[index]
        self.assertTrue(trades)
        first_entry = trades[0]["entry_time"]
        slow_ready_time = int(frame.index[3].timestamp() * 1000)
        self.assertGreaterEqual(first_entry, slow_ready_time)

    def test_insufficient_bars_raises(self) -> None:
        frame = make_frame([100, 101, 102])
        with self.assertRaises(Exception):
            self.run_engine(frame, fast=2, slow=10)


if __name__ == "__main__":
    unittest.main()
