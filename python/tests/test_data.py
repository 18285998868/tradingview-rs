from __future__ import annotations

import tempfile
import unittest
import zipfile
from datetime import datetime, timezone
from pathlib import Path

from backtest_worker.data import DataError, load_klines


def ms(year: int, month: int, day: int, hour: int = 0) -> int:
    return int(datetime(year, month, day, hour, tzinfo=timezone.utc).timestamp() * 1000)


def write_zip(directory: Path, name: str, rows: list[tuple[int, float]], header: bool = False) -> None:
    directory.mkdir(parents=True, exist_ok=True)
    lines = []
    if header:
        lines.append("open_time,open,high,low,close,volume,close_time,quote_volume,count,ignore")
    for open_time, close in rows:
        lines.append(
            f"{open_time},{close},{close + 1},{close - 1},{close},10,"
            f"{open_time + 59_999},100,1,0"
        )
    csv_name = name.removesuffix(".zip") + ".csv"
    with zipfile.ZipFile(directory / name, "w", zipfile.ZIP_DEFLATED) as archive:
        archive.writestr(csv_name, "\n".join(lines) + "\n")


class DataTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.monthly = self.root / "binance/data/spot/monthly/klines/BTCUSDT/1h"
        self.daily = self.root / "binance/data/spot/daily/klines/BTCUSDT/1h"

    def tearDown(self) -> None:
        self.temp.cleanup()

    def test_daily_archive_overrides_monthly(self) -> None:
        write_zip(
            self.monthly,
            "BTCUSDT-1h-2024-01.zip",
            [(ms(2024, 1, 1), 100.0), (ms(2024, 1, 2), 101.0), (ms(2024, 1, 3), 102.0)],
        )
        write_zip(self.daily, "BTCUSDT-1h-2024-01-02.zip", [(ms(2024, 1, 2), 201.0)], header=True)
        loaded = load_klines(self.root, "BTCUSDT", "1h")
        self.assertEqual(loaded.rows, 3)
        self.assertEqual(loaded.files, 2)
        self.assertEqual(list(loaded.frame["close"]), [100.0, 201.0, 102.0])
        self.assertEqual(loaded.first_ms, ms(2024, 1, 1))
        self.assertEqual(loaded.last_ms, ms(2024, 1, 3))

    def test_start_end_filter(self) -> None:
        write_zip(
            self.monthly,
            "BTCUSDT-1h-2024-01.zip",
            [(ms(2024, 1, 1), 100.0), (ms(2024, 1, 2), 101.0), (ms(2024, 1, 3), 102.0)],
        )
        loaded = load_klines(
            self.root,
            "BTCUSDT",
            "1h",
            start_ms=ms(2024, 1, 2),
            end_ms=ms(2024, 1, 3),
        )
        self.assertEqual(loaded.rows, 1)
        self.assertEqual(loaded.first_ms, ms(2024, 1, 2))

    def test_range_only_reads_overlapping_monthly_archives(self) -> None:
        write_zip(
            self.monthly,
            "BTCUSDT-1h-2024-01.zip",
            [(ms(2024, 1, 1), 100.0), (ms(2024, 1, 2), 101.0)],
        )
        write_zip(
            self.monthly,
            "BTCUSDT-1h-2024-02.zip",
            [(ms(2024, 2, 1), 200.0), (ms(2024, 2, 2), 201.0)],
        )
        loaded = load_klines(
            self.root,
            "BTCUSDT",
            "1h",
            start_ms=ms(2024, 2, 1),
            end_ms=ms(2024, 3, 1),
        )
        self.assertEqual(loaded.rows, 2)
        self.assertEqual(loaded.files, 1)
        self.assertEqual(loaded.first_ms, ms(2024, 2, 1))

    def test_range_only_reads_overlapping_daily_archives(self) -> None:
        write_zip(self.daily, "BTCUSDT-1h-2024-01-02.zip", [(ms(2024, 1, 2), 100.0)])
        write_zip(self.daily, "BTCUSDT-1h-2024-02-02.zip", [(ms(2024, 2, 2), 200.0)])
        loaded = load_klines(
            self.root,
            "BTCUSDT",
            "1h",
            start_ms=ms(2024, 2, 1),
            end_ms=ms(2024, 3, 1),
        )
        self.assertEqual(loaded.rows, 1)
        self.assertEqual(loaded.files, 1)
        self.assertEqual(loaded.first_ms, ms(2024, 2, 2))

    def test_range_without_overlapping_archives_raises(self) -> None:
        write_zip(self.monthly, "BTCUSDT-1h-2024-01.zip", [(ms(2024, 1, 1), 100.0)])
        with self.assertRaises(DataError):
            load_klines(
                self.root,
                "BTCUSDT",
                "1h",
                start_ms=ms(2024, 2, 1),
                end_ms=ms(2024, 3, 1),
            )

    def test_microsecond_timestamps_are_normalized(self) -> None:
        # Binance 现货归档从 2025-01-01 起使用微秒。
        write_zip(
            self.monthly,
            "BTCUSDT-1h-2025-01.zip",
            [(ms(2025, 1, 1) * 1000, 100.0), (ms(2025, 1, 1, 1) * 1000, 101.0)],
        )
        loaded = load_klines(self.root, "BTCUSDT", "1h")
        self.assertEqual(loaded.first_ms, ms(2025, 1, 1))
        self.assertEqual(loaded.last_ms, ms(2025, 1, 1, 1))
        self.assertEqual(loaded.rows, 2)

        filtered = load_klines(
            self.root,
            "BTCUSDT",
            "1h",
            start_ms=ms(2025, 1, 1, 1),
        )
        self.assertEqual(filtered.rows, 1)
        self.assertEqual(filtered.first_ms, ms(2025, 1, 1, 1))

    def test_missing_archives_raise(self) -> None:
        with self.assertRaises(DataError):
            load_klines(self.root, "BTCUSDT", "1h")

    def test_bad_zip_raises(self) -> None:
        self.monthly.mkdir(parents=True)
        (self.monthly / "BTCUSDT-1h-2024-01.zip").write_bytes(b"not a zip")
        with self.assertRaises(DataError):
            load_klines(self.root, "BTCUSDT", "1h")

    def test_duplicate_in_same_archive_raises(self) -> None:
        write_zip(
            self.monthly,
            "BTCUSDT-1h-2024-01.zip",
            [(ms(2024, 1, 1), 100.0), (ms(2024, 1, 1), 101.0)],
        )
        with self.assertRaises(DataError):
            load_klines(self.root, "BTCUSDT", "1h")


if __name__ == "__main__":
    unittest.main()
