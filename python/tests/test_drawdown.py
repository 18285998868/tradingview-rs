from __future__ import annotations

import unittest

import numpy as np

from backtest_worker.drawdown import compute_drawdown


DAY = 86_400_000
T0 = 1_700_000_000_000


class DrawdownTest(unittest.TestCase):
    def test_monotonic_up_has_no_drawdown(self) -> None:
        result = compute_drawdown([T0, T0 + DAY, T0 + 2 * DAY], [100, 110, 120])
        self.assertEqual(result.events, [])
        self.assertEqual(result.max_drawdown_pct, 0.0)
        self.assertEqual(result.avg_drawdown_pct, 0.0)
        self.assertEqual(result.drawdown_time_pct, 0.0)
        np.testing.assert_allclose(result.underwater_pct, [0.0, 0.0, 0.0])

    def test_single_recovered_event(self) -> None:
        times = [T0 + i * DAY for i in range(5)]
        result = compute_drawdown(times, [100, 120, 90, 100, 130])
        self.assertEqual(len(result.events), 1)
        event = result.events[0]
        self.assertEqual(event.peak_time, times[1])
        self.assertEqual(event.trough_time, times[2])
        self.assertEqual(event.recovery_time, times[4])
        self.assertAlmostEqual(event.depth_pct, 25.0, places=9)
        self.assertEqual(event.duration_ms, 3 * DAY)
        self.assertEqual(event.recovery_ms, 2 * DAY)
        self.assertTrue(event.recovered)
        self.assertAlmostEqual(result.drawdown_time_pct, 40.0, places=9)

    def test_multiple_events(self) -> None:
        times = [T0 + i * DAY for i in range(6)]
        result = compute_drawdown(times, [100, 110, 100, 120, 90, 120])
        self.assertEqual(len(result.events), 2)
        self.assertAlmostEqual(result.events[0].depth_pct, 100 / 11, places=9)
        self.assertAlmostEqual(result.events[1].depth_pct, 25.0, places=9)
        self.assertEqual(result.events[0].recovery_time, times[3])
        self.assertEqual(result.events[1].recovery_time, times[5])
        self.assertAlmostEqual(result.avg_drawdown_pct, 17.045454545, places=6)

    def test_open_event_is_marked(self) -> None:
        times = [T0 + i * DAY for i in range(4)]
        result = compute_drawdown(times, [100, 120, 80, 90])
        self.assertEqual(len(result.events), 1)
        event = result.events[0]
        self.assertFalse(event.recovered)
        self.assertIsNone(event.recovery_time)
        self.assertIsNone(event.recovery_ms)
        self.assertEqual(event.duration_ms, 2 * DAY)
        self.assertAlmostEqual(event.depth_pct, 100 / 3, places=9)

    def test_flat_series(self) -> None:
        result = compute_drawdown([T0, T0 + DAY], [100, 100])
        self.assertEqual(result.events, [])

    def test_rejects_unsorted_times(self) -> None:
        with self.assertRaises(ValueError):
            compute_drawdown([T0 + DAY, T0], [100, 90])

    def test_rejects_non_positive_equity(self) -> None:
        with self.assertRaises(ValueError):
            compute_drawdown([T0, T0 + DAY], [100, 0])


if __name__ == "__main__":
    unittest.main()
