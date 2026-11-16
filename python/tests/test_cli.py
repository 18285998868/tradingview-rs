from __future__ import annotations

import tempfile
import unittest
import zipfile
from datetime import datetime, timezone
from pathlib import Path

try:
    import vectorbt  # noqa: F401

    HAS_VECTORBT = True
except ImportError:
    HAS_VECTORBT = False

from backtest_worker.cli import execute, validate_request


def ms(day: int) -> int:
    return int(datetime(2024, 1, day, tzinfo=timezone.utc).timestamp() * 1000)


def write_monthly(root: Path, closes: list[float]) -> None:
    directory = root / "binance/data/spot/monthly/klines/BTCUSDT/1d"
    directory.mkdir(parents=True, exist_ok=True)
    lines = []
    for index, close in enumerate(closes, start=1):
        open_time = ms(index)
        lines.append(
            f"{open_time},{close},{close + 1},{close - 1},{close},10,"
            f"{open_time + 86_399_999},100,1,0"
        )
    with zipfile.ZipFile(directory / "BTCUSDT-1d-2024-01.zip", "w") as archive:
        archive.writestr("BTCUSDT-1d-2024-01.csv", "\n".join(lines) + "\n")


def request(root: Path, **overrides: object) -> dict[str, object]:
    payload: dict[str, object] = {
        "data_dir": str(root),
        "market_type": "spot",
        "pair": "BTCUSDT",
        "timeframe": "1d",
        "strategy": "dual_ma",
        "params_list": [{"fast": 2, "slow": 4}],
        "fee_bps": 10,
        "slippage_bps": 5,
        "initial_balance": 10_000,
        "max_curve_points": 50,
        "max_trades": 1000,
    }
    payload.update(overrides)
    return payload


class ValidationTest(unittest.TestCase):
    def test_rejects_non_spot(self) -> None:
        with self.assertRaises(ValueError):
            validate_request(
                {
                    "data_dir": "/tmp",
                    "pair": "BTCUSDT",
                    "timeframe": "1h",
                    "params_list": [{"fast": 2, "slow": 4}],
                    "market_type": "futures",
                }
            )

    def test_rejects_duplicate_params(self) -> None:
        with self.assertRaises(ValueError):
            validate_request(
                {
                    "data_dir": "/tmp",
                    "pair": "BTCUSDT",
                    "timeframe": "1h",
                    "params_list": [{"fast": 2, "slow": 4}, {"fast": 2, "slow": 4}],
                }
            )


@unittest.skipUnless(HAS_VECTORBT, "vectorbt not installed")
class ExecuteTest(unittest.TestCase):
    def test_execute_returns_contract_shape(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            write_monthly(root, [100, 101, 102, 103, 104, 103, 102, 101, 105, 107])
            result = execute(request(root))
            self.assertEqual(result["strategy"], "dual_ma")
            self.assertEqual(result["rows"], 10)
            self.assertEqual(len(result["runs"]), 1)  # type: ignore[arg-type]
            run = result["runs"][0]  # type: ignore[index]
            self.assertIsNone(run["error"])
            self.assertEqual(run["params_text"], "fast=2,slow=4")
            self.assertLessEqual(len(run["equity"]), 50)
            self.assertIn("max_drawdown_pct", run["metrics"])
            self.assertEqual(run["equity"][0][0], run["first_time"])
            self.assertEqual(run["underwater"][0][0], run["first_time"])

    def test_one_bad_param_does_not_kill_batch(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            write_monthly(root, [100, 101, 102, 103, 104, 105])
            payload = request(
                root,
                params_list=[{"fast": 2, "slow": 4}, {"fast": 2, "slow": 50}],
            )
            result = execute(payload)
            runs = result["runs"]  # type: ignore[index]
            self.assertEqual(len(runs), 2)
            self.assertIsNone(runs[0]["error"])
            self.assertIsNotNone(runs[1]["error"])


if __name__ == "__main__":
    unittest.main()
