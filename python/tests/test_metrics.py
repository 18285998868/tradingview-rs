from __future__ import annotations

import unittest

from backtest_worker.metrics import compute_metrics


class MetricsTest(unittest.TestCase):
    def metrics(self, **overrides: object) -> dict[str, object]:
        base = {
            "timeframe": "1d",
            "equity": [100, 110, 120],
            "initial_balance": 100,
            "trade_pnls": [10, -5, 15],
            "trade_returns": [0.1, -0.05, 0.15],
            "max_drawdown_pct": 4.0,
            "avg_drawdown_pct": 2.0,
            "drawdown_count": 2,
            "longest_drawdown_ms": 3 * 86_400_000,
            "longest_recovery_ms": 2 * 86_400_000,
            "drawdown_time_pct": 25.0,
        }
        base.update(overrides)
        return compute_metrics(**base)  # type: ignore[arg-type]

    def test_basic_values(self) -> None:
        result = self.metrics()
        self.assertEqual(result["num_trades"], 3)
        self.assertAlmostEqual(result["win_rate"], 200 / 3, places=9)
        self.assertAlmostEqual(result["total_return_pct"], 20.0, places=9)
        self.assertAlmostEqual(result["profit_factor"], 25 / 5, places=9)
        self.assertAlmostEqual(result["expectancy"], 0.2 / 3, places=9)
        self.assertEqual(result["max_drawdown_pct"], 4.0)
        self.assertEqual(result["drawdown_count"], 2)
        self.assertEqual(result["longest_drawdown_days"], 3.0)
        self.assertEqual(result["longest_recovery_days"], 2.0)
        self.assertEqual(result["bars"], 3)

    def test_zero_volatility_has_no_sharpe(self) -> None:
        result = self.metrics(equity=[100, 100, 100], trade_pnls=[], trade_returns=[])
        self.assertIsNone(result["sharpe_ratio"])
        self.assertIsNone(result["sortino_ratio"])
        self.assertIsNone(result["win_rate"])
        self.assertIsNone(result["profit_factor"])

    def test_expected_value_with_no_losses_is_undefined(self) -> None:
        result = self.metrics(trade_pnls=[1, 2], trade_returns=[0.1, 0.2])
        self.assertIsNone(result["profit_factor"])

    def test_sharpe_uses_documented_annualization(self) -> None:
        result = self.metrics(equity=[100, 101, 102, 103], timeframe="1d")
        self.assertIsNotNone(result["sharpe_ratio"])
        self.assertAlmostEqual(result["periods_per_year"], 365.0)


if __name__ == "__main__":
    unittest.main()
