from __future__ import annotations

import unittest

from backtest_worker.curve import select_curve_indices


class CurveTest(unittest.TestCase):
    def test_short_series_keeps_all_points(self) -> None:
        self.assertEqual(select_curve_indices(20, 100, []), list(range(20)))

    def test_limits_points_and_keeps_ends(self) -> None:
        selected = select_curve_indices(10_000, 200, [(5_000, 30.0)])
        self.assertLessEqual(len(selected), 200)
        self.assertEqual(selected, sorted(set(selected)))
        self.assertEqual(selected[0], 0)
        self.assertEqual(selected[-1], 9_999)

    def test_keeps_deepest_troughs(self) -> None:
        events = [(1_234, 10.0), (7_654, 40.0), (9_000, 20.0)]
        selected = select_curve_indices(10_000, 100, events)
        self.assertIn(7_654, selected)
        self.assertIn(1_234, selected)
        self.assertIn(9_000, selected)

    def test_ignores_out_of_range_events(self) -> None:
        selected = select_curve_indices(1_000, 50, [(-1, 99.0), (1_000, 99.0)])
        self.assertEqual(selected[0], 0)
        self.assertEqual(selected[-1], 999)
        self.assertLessEqual(len(selected), 50)


if __name__ == "__main__":
    unittest.main()
