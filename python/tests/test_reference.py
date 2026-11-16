"""独立参考实现与生产引擎的对账测试。

覆盖：

- 双均线引擎：合成随机游走，0 成本与 10+5 bps 两组，逐笔交易、权益、回撤、
  指标全部与显式现金/持仓循环对比；
- 回撤：10 组固定权益序列的逐事件对账；
- 成本公式：单笔交易的手算核对；
- 无前视：首笔入场不早于 slow 窗口；
- 降采样：2000 点水下曲线与完整曲线的误差；
- 确定性：同一请求多次运行的结果哈希一致；
- 真实数据：设置 `TRADINGVIEW_REAL_DATA` 后额外跑 BTCUSDT 1h 对账与
  1m->1h 聚合一致性。
"""

from __future__ import annotations

import hashlib
import json
import os
import unittest

import numpy as np
import pandas as pd

try:
    import vectorbt  # noqa: F401

    HAS_VECTORBT = True
except ImportError:
    HAS_VECTORBT = False

from backtest_worker.data import load_klines
from backtest_worker.drawdown import compute_drawdown
from backtest_worker.engine import run_dual_ma
from reference import compute_drawdown_reference, run_dual_ma_reference

REAL_DATA_ROOT = os.environ.get("TRADINGVIEW_REAL_DATA")


def make_random_walk(count: int = 3000, seed: int = 7) -> pd.DataFrame:
    """确定性随机游走，OHLC 都有效且 open != close。"""

    rng = np.random.default_rng(seed)
    closes = 100.0 * np.exp(np.cumsum(rng.normal(0.0, 0.01, count)))
    opens = np.concatenate(([100.0], closes[:-1])) * (1.0 + rng.normal(0.0, 0.0005, count))
    highs = np.maximum(opens, closes) * (1.0 + np.abs(rng.normal(0.0, 0.002, count)))
    lows = np.minimum(opens, closes) * (1.0 - np.abs(rng.normal(0.0, 0.002, count)))
    index = pd.date_range("2024-01-01", periods=count, freq="h", tz="UTC")
    return pd.DataFrame(
        {
            "open": opens,
            "high": highs,
            "low": lows,
            "close": closes,
            "volume": rng.uniform(1.0, 100.0, count),
        },
        index=index,
    )


def make_frame(closes: list[float]) -> pd.DataFrame:
    index = pd.date_range("2024-01-01", periods=len(closes), freq="h", tz="UTC")
    array = np.asarray(closes, dtype=np.float64)
    return pd.DataFrame(
        {
            "open": array + 0.5,
            "high": array + 2.0,
            "low": array - 2.0,
            "close": array,
            "volume": np.full(len(closes), 10.0),
        },
        index=index,
    )


def canonical_hash(result: dict[str, object]) -> str:
    payload = {
        "metrics": result["metrics"],
        "trades": result["trades"],
        "equity": result["equity"],
        "underwater": result["underwater"],
        "drawdown_events": result["drawdown_events"],
    }
    text = json.dumps(payload, sort_keys=True, separators=(",", ":"), allow_nan=False)
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


class DrawdownReconciliationTest(unittest.TestCase):
    """生产回撤实现 vs 独立事件循环。"""

    def compare_case(self, name: str, equity: list[float]) -> None:
        times = [1_700_000_000_000 + index * 3_600_000 for index in range(len(equity))]
        produced = compute_drawdown(times, equity)
        reference = compute_drawdown_reference(times, equity)

        ctx = f"case={name}"
        self.assertEqual(len(produced.events), len(reference.events), ctx)
        np.testing.assert_allclose(
            produced.underwater_pct,
            reference.underwater_pct,
            rtol=0.0,
            atol=1e-9,
            err_msg=ctx,
        )
        self.assertAlmostEqual(
            produced.drawdown_time_pct, reference.drawdown_time_pct, places=12, msg=ctx
        )
        self.assertAlmostEqual(
            produced.max_drawdown_pct, reference.max_drawdown_pct, places=12, msg=ctx
        )
        self.assertAlmostEqual(
            produced.avg_drawdown_pct, reference.avg_drawdown_pct, places=12, msg=ctx
        )
        self.assertEqual(produced.longest_drawdown_ms, reference.longest_drawdown_ms, ctx)
        self.assertEqual(produced.longest_recovery_ms, reference.longest_recovery_ms, ctx)

        for produced_event, reference_event in zip(produced.events, reference.events):
            for key in (
                "peak_time",
                "trough_time",
                "recovery_time",
                "duration_ms",
                "recovery_ms",
                "recovered",
                "peak_index",
                "trough_index",
            ):
                self.assertEqual(
                    getattr(produced_event, key), reference_event[key], f"{ctx} {key}"
                )
            self.assertAlmostEqual(
                produced_event.depth_pct,
                reference_event["depth_pct"],
                places=12,
                msg=f"{ctx} depth_pct",
            )

    def test_fixed_series(self) -> None:
        cases: dict[str, list[float]] = {
            "monotonic_up": [100.0, 101.0, 102.0, 103.0, 104.0],
            "monotonic_down": [104.0, 103.0, 102.0, 101.0, 100.0],
            "v_shape": [100.0, 90.0, 95.0, 100.0, 105.0],
            "double_dip": [100.0, 80.0, 100.0, 70.0, 100.0],
            "plateau": [100.0, 100.0, 99.0, 100.0, 100.0],
            "recover_at_last": [100.0, 90.0, 80.0, 95.0, 100.0],
            "start_in_drawdown": [100.0, 99.0, 98.0, 99.0, 100.0],
            "flat": [100.0] * 6,
            "unrecovered_after_dip": [100.0, 110.0, 90.0, 95.0],
            "tiny_moves": [100.0, 100.0000000001, 99.9999999999, 100.0],
        }
        for name, equity in cases.items():
            self.compare_case(name, equity)

    def test_random_series(self) -> None:
        for seed in range(10):
            rng = np.random.default_rng(1000 + seed)
            equity = (100.0 * np.exp(np.cumsum(rng.normal(0.0, 0.02, 500)))).tolist()
            self.compare_case(f"random_{seed}", equity)


@unittest.skipUnless(HAS_VECTORBT, "vectorbt not installed")
class EngineReconciliationTest(unittest.TestCase):
    def run_engine(self, frame: pd.DataFrame, fast: int, slow: int) -> dict[str, object]:
        return run_dual_ma(
            frame=frame,
            params={"fast": fast, "slow": slow},
            timeframe="1h",
            fee_bps=self.fee_bps,
            slippage_bps=self.slippage_bps,
            initial_balance=10_000.0,
            max_curve_points=len(frame),
            max_trades=1_000_000,
        )

    def reconcile(self, frame: pd.DataFrame, fast: int, slow: int) -> None:
        produced = self.run_engine(frame, fast, slow)
        reference = run_dual_ma_reference(
            frame=frame,
            params={"fast": fast, "slow": slow},
            timeframe="1h",
            fee_bps=self.fee_bps,
            slippage_bps=self.slippage_bps,
            initial_balance=10_000.0,
        )

        produced_trades = produced["trades"]
        reference_trades = reference["trades"]
        self.assertEqual(len(produced_trades), len(reference_trades))

        for index, (got, want) in enumerate(zip(produced_trades, reference_trades)):
            ctx = f"trade {index}"
            self.assertEqual(got["seq"], index, ctx)
            self.assertEqual(got["side"], "long", ctx)
            self.assertEqual(got["status"], want.status, ctx)
            self.assertEqual(got["entry_time"], reference["times"][want.entry_index], ctx)
            self.assertEqual(got["exit_time"], reference["times"][want.exit_index], ctx)
            self.assertEqual(got["bars_held"], want.exit_index - want.entry_index, ctx)
            self.assertAlmostEqual(got["entry_price"], want.entry_price, places=10, msg=ctx)
            self.assertAlmostEqual(got["exit_price"], want.exit_price, places=10, msg=ctx)
            self.assertAlmostEqual(got["qty"], want.qty, places=10, msg=ctx)
            self.assertAlmostEqual(got["pnl"], want.pnl, places=8, msg=ctx)
            self.assertAlmostEqual(
                got["pnl_pct"], want.trade_return * 100.0, places=10, msg=ctx
            )

        self.assertEqual(produced["equity"], [[t, float(v)] for t, v in zip(reference["times"], reference["equity"])])
        self.assertEqual(
            produced["underwater"],
            [[t, float(v)] for t, v in zip(reference["times"], reference["drawdown"].underwater_pct)],
        )

        produced_metrics = produced["metrics"]
        for key, want in reference["metrics"].items():
            got = produced_metrics[key]
            if want is None:
                self.assertIsNone(got, key)
            elif isinstance(want, float):
                self.assertAlmostEqual(got, want, places=8, msg=key)
            else:
                self.assertEqual(got, want, key)

        produced_events = produced["drawdown_events"]
        self.assertEqual(len(produced_events), len(reference["drawdown"].events))
        for index, (got, want) in enumerate(
            zip(produced_events, reference["drawdown"].events)
        ):
            ctx = f"event {index}"
            self.assertEqual(got["seq"], index, ctx)
            for key in (
                "peak_time",
                "trough_time",
                "recovery_time",
                "duration_ms",
                "recovery_ms",
                "recovered",
            ):
                self.assertEqual(got[key], want[key], f"{ctx} {key}")
            self.assertAlmostEqual(got["depth_pct"], want["depth_pct"], places=10, msg=ctx)

    def test_zero_cost(self) -> None:
        self.fee_bps = 0.0
        self.slippage_bps = 0.0
        self.reconcile(make_random_walk(3000, seed=7), fast=10, slow=30)

    def test_with_costs(self) -> None:
        self.fee_bps = 10.0
        self.slippage_bps = 5.0
        self.reconcile(make_random_walk(3000, seed=11), fast=5, slow=20)

    def test_equity_is_marked_to_close_not_open(self) -> None:
        """权益必须按收盘价估值；成交仍按开盘价。"""

        frame = make_frame([100.0, 101.0, 102.0, 103.0, 104.0, 105.0, 106.0, 107.0])
        frame["close"] = [100.0, 151.0, 152.0, 203.0, 254.0, 305.0, 306.0, 307.0]
        produced = run_dual_ma(
            frame=frame,
            params={"fast": 2, "slow": 3},
            timeframe="1h",
            fee_bps=0.0,
            slippage_bps=0.0,
            initial_balance=10_000.0,
            max_curve_points=100,
            max_trades=100,
        )
        trades = produced["trades"]
        self.assertTrue(trades)
        entry_time = trades[0]["entry_time"]
        index = list(frame.index).index(pd.Timestamp(entry_time, unit="ms", tz="UTC"))
        self.assertAlmostEqual(
            produced["equity"][index][1] / 10_000.0,
            frame["close"].iloc[index] / frame["open"].iloc[index],
            places=9,
        )


class CostFormulaTest(unittest.TestCase):
    """单笔交易的手算核对。"""

    @unittest.skipUnless(HAS_VECTORBT, "vectorbt not installed")
    def test_single_trade_formula(self) -> None:
        closes = [100.0] * 5 + [101, 102, 103, 104, 105, 106, 107, 108]
        closes += [107, 106, 105, 104, 103, 102, 101, 100, 99, 98]
        closes += [97.0] * 10
        frame = make_frame(closes)

        fee_bps = 10.0
        slippage_bps = 5.0
        fee = fee_bps / 10_000.0
        slippage = slippage_bps / 10_000.0
        result = run_dual_ma(
            frame=frame,
            params={"fast": 2, "slow": 4},
            timeframe="1h",
            fee_bps=fee_bps,
            slippage_bps=slippage_bps,
            initial_balance=10_000.0,
            max_curve_points=len(frame),
            max_trades=100,
        )
        trades = result["trades"]
        self.assertEqual(len(trades), 1)
        trade = trades[0]

        entry_index = list(frame.index).index(pd.Timestamp(trade["entry_time"], unit="ms", tz="UTC"))
        exit_index = list(frame.index).index(pd.Timestamp(trade["exit_time"], unit="ms", tz="UTC"))
        entry_price = float(frame["open"].iloc[entry_index]) * (1.0 + slippage)
        exit_price = float(frame["open"].iloc[exit_index]) * (1.0 - slippage)
        notional = 10_000.0 / (1.0 + fee)
        qty = notional / entry_price
        entry_fee = notional * fee
        exit_fee = qty * exit_price * fee
        pnl = qty * (exit_price - entry_price) - entry_fee - exit_fee

        self.assertAlmostEqual(trade["entry_price"], entry_price, places=10)
        self.assertAlmostEqual(trade["exit_price"], exit_price, places=10)
        self.assertAlmostEqual(trade["qty"], qty, places=10)
        self.assertAlmostEqual(trade["pnl"], pnl, places=8)
        self.assertAlmostEqual(trade["pnl_pct"], pnl / notional * 100.0, places=10)


@unittest.skipUnless(HAS_VECTORBT, "vectorbt not installed")
class NoLookaheadTest(unittest.TestCase):
    def test_first_entry_after_slow_window(self) -> None:
        frame = make_random_walk(1000, seed=23)
        slow = 30
        result = run_dual_ma(
            frame=frame,
            params={"fast": 10, "slow": slow},
            timeframe="1h",
            fee_bps=0.0,
            slippage_bps=0.0,
            initial_balance=10_000.0,
            max_curve_points=2000,
            max_trades=10_000,
        )
        trades = result["trades"]
        self.assertTrue(trades)
        self.assertGreaterEqual(trades[0]["entry_time"], int(frame.index[slow].timestamp() * 1000))
        for index, trade in enumerate(trades):
            entry_index = list(frame.index).index(
                pd.Timestamp(trade["entry_time"], unit="ms", tz="UTC")
            )
            # 信号在 T 收盘产生，成交在 T+1 开盘，因此最早也只能是 slow。
            self.assertGreaterEqual(entry_index, slow, f"trade {index}")


@unittest.skipUnless(HAS_VECTORBT, "vectorbt not installed")
class DownsampleTest(unittest.TestCase):
    def test_underwater_downsample_keeps_extremes(self) -> None:
        frame = make_random_walk(20_000, seed=31)
        full = run_dual_ma(
            frame=frame,
            params={"fast": 10, "slow": 30},
            timeframe="1h",
            fee_bps=10.0,
            slippage_bps=5.0,
            initial_balance=10_000.0,
            max_curve_points=len(frame),
            max_trades=1_000_000,
        )
        sampled = run_dual_ma(
            frame=frame,
            params={"fast": 10, "slow": 30},
            timeframe="1h",
            fee_bps=10.0,
            slippage_bps=5.0,
            initial_balance=10_000.0,
            max_curve_points=500,
            max_trades=1_000_000,
        )
        self.assertLessEqual(len(sampled["underwater"]), 500)
        full_at = {time: value for time, value in full["underwater"]}
        for time, value in sampled["underwater"]:
            self.assertAlmostEqual(value, full_at[time], places=9)
        self.assertAlmostEqual(
            min(value for _, value in sampled["underwater"]),
            min(value for _, value in full["underwater"]),
            places=9,
        )


@unittest.skipUnless(HAS_VECTORBT, "vectorbt not installed")
class DeterminismTest(unittest.TestCase):
    def test_three_runs_hash_identical(self) -> None:
        frame = make_random_walk(1000, seed=41)
        hashes = []
        for _ in range(3):
            result = run_dual_ma(
                frame=frame,
                params={"fast": 5, "slow": 20},
                timeframe="1h",
                fee_bps=10.0,
                slippage_bps=5.0,
                initial_balance=10_000.0,
                max_curve_points=500,
                max_trades=10_000,
            )
            hashes.append(canonical_hash(result))
        self.assertEqual(len(set(hashes)), 1, hashes)


@unittest.skipUnless(HAS_VECTORBT, "vectorbt not installed")
@unittest.skipUnless(REAL_DATA_ROOT, "TRADINGVIEW_REAL_DATA not set")
class RealDataReconciliationTest(unittest.TestCase):
    def setUp(self) -> None:
        self.data_root = REAL_DATA_ROOT
        self.start_ms = int(pd.Timestamp("2024-01-01", tz="UTC").timestamp() * 1000)
        self.end_ms = int(pd.Timestamp("2024-07-01", tz="UTC").timestamp() * 1000)

    def test_btcusdt_1h_engine_vs_reference(self) -> None:
        loaded = load_klines(
            self.data_root, "BTCUSDT", "1h", self.start_ms, self.end_ms
        )
        frame = loaded.frame
        self.assertGreater(len(frame), 4000)
        produced = run_dual_ma(
            frame=frame,
            params={"fast": 10, "slow": 30},
            timeframe="1h",
            fee_bps=10.0,
            slippage_bps=5.0,
            initial_balance=10_000.0,
            max_curve_points=len(frame),
            max_trades=1_000_000,
        )
        reference = run_dual_ma_reference(
            frame=frame,
            params={"fast": 10, "slow": 30},
            timeframe="1h",
            fee_bps=10.0,
            slippage_bps=5.0,
            initial_balance=10_000.0,
        )
        self.assertEqual(len(produced["trades"]), len(reference["trades"]))
        self.assertAlmostEqual(
            produced["metrics"]["total_return_pct"],
            reference["metrics"]["total_return_pct"],
            places=8,
        )
        self.assertAlmostEqual(
            produced["metrics"]["max_drawdown_pct"],
            reference["metrics"]["max_drawdown_pct"],
            places=8,
        )
        np.testing.assert_allclose(
            [value for _, value in produced["equity"]],
            reference["equity"],
            rtol=0.0,
            atol=1e-9,
        )

        # R04：2000 点降采样曲线逐点回查完整曲线。
        sampled = run_dual_ma(
            frame=frame,
            params={"fast": 10, "slow": 30},
            timeframe="1h",
            fee_bps=10.0,
            slippage_bps=5.0,
            initial_balance=10_000.0,
            max_curve_points=2000,
            max_trades=1_000_000,
        )
        self.assertLessEqual(len(sampled["underwater"]), 2000)
        full_at = {
            time: value
            for time, value in zip(
                reference["times"], reference["drawdown"].underwater_pct
            )
        }
        for time, value in sampled["underwater"]:
            self.assertAlmostEqual(value, full_at[time], places=9)
        self.assertAlmostEqual(
            min(value for _, value in sampled["underwater"]),
            min(full_at.values()),
            places=9,
        )

    def test_1m_aggregates_to_native_1h_and_1d(self) -> None:
        end_ms = int(pd.Timestamp("2024-02-01", tz="UTC").timestamp() * 1000)
        minutes = load_klines(self.data_root, "BTCUSDT", "1m", self.start_ms, end_ms)
        for timeframe, rule, minimum in (("1h", "1h", 500), ("1d", "1D", 20)):
            native = load_klines(self.data_root, "BTCUSDT", timeframe, self.start_ms, end_ms)
            aggregated = minutes.frame.resample(rule).agg(
                {
                    "open": "first",
                    "high": "max",
                    "low": "min",
                    "close": "last",
                    "volume": "sum",
                }
            )
            # 只比较两边都完整的整点/整日区间。
            joined = native.frame.join(
                aggregated, how="inner", lsuffix="_native", rsuffix="_agg"
            )
            joined = joined.dropna()
            self.assertGreater(len(joined), minimum, timeframe)
            for column in ("open", "high", "low", "close"):
                np.testing.assert_allclose(
                    joined[f"{column}_native"],
                    joined[f"{column}_agg"],
                    rtol=1e-12,
                    atol=0.0,
                    err_msg=f"{timeframe} {column}",
                )
            relative = (
                (joined["volume_native"] - joined["volume_agg"]).abs()
                / joined["volume_native"].abs().clip(lower=1e-12)
            )
            self.assertLessEqual(float(relative.max()), 1e-6, timeframe)


if __name__ == "__main__":
    unittest.main()
