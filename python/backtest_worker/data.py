"""Binance 现货归档读取。

目录布局与 `data-reader` 一致：

    {data_dir}/binance/data/spot/{daily,monthly}/klines/{symbol}/{interval}/*.zip

日归档覆盖到的 UTC 日期优先于月归档；月归档中同一天的行会被丢弃。时间戳统一
为毫秒。读取阶段只做结构校验，不修补缺口。
"""

from __future__ import annotations

import csv
import io
import math
import re
import zipfile
from dataclasses import dataclass
from datetime import date, datetime, timezone
from pathlib import Path
from typing import Iterable, Iterator

import pandas as pd

_MONTHLY_RE = re.compile(r"-(?P<year>\d{4})-(?P<month>\d{2})\.zip$")
_DAILY_RE = re.compile(r"-(?P<year>\d{4})-(?P<month>\d{2})-(?P<day>\d{2})\.zip$")

# Binance 现货归档从 2025-01-01 起把 open_time/close_time 改为微秒；期货仍是毫秒。
# 毫秒时间戳要到公元 5138 年才会达到 1e14，用数量级即可安全区分。
_MICROSECOND_THRESHOLD = 10**14


def _open_time_ms(value: int) -> int:
    """把 Binance 归档时间戳统一成毫秒。"""

    return value // 1000 if value >= _MICROSECOND_THRESHOLD else value


class DataError(RuntimeError):
    """数据缺失、归档非法或数值不可用。"""


@dataclass(frozen=True)
class LoadedKlines:
    """读取结果：DataFrame 索引为 UTC 时间，列为 OHLCV。"""

    frame: pd.DataFrame
    files: int
    rows: int
    first_ms: int
    last_ms: int


def archive_dirs(data_dir: str | Path, pair: str, interval: str) -> tuple[Path, Path]:
    """返回 `(monthly_dir, daily_dir)`；不检查存在性。"""

    root = Path(data_dir) / "binance" / "data" / "spot"
    return (
        root / "monthly" / "klines" / pair / interval,
        root / "daily" / "klines" / pair / interval,
    )


def _zip_files(directory: Path, pattern: re.Pattern[str]) -> list[tuple[date, Path]]:
    if not directory.is_dir():
        return []
    out: list[tuple[date, Path]] = []
    for path in sorted(directory.glob("*.zip")):
        match = pattern.search(path.name)
        if not match:
            continue
        parts = {k: int(v) for k, v in match.groupdict().items()}
        out.append((date(parts["year"], parts["month"], parts.get("day", 1)), path))
    return out


def _iter_rows(path: Path, start_ms: int | None, end_ms: int | None) -> Iterator[tuple[int, float, float, float, float, float]]:
    """逐行读取一个 zip 内唯一的 CSV。

    只取前 6 列（open_time, open, high, low, close, volume）。Binance 不同数据集
    可能在末尾追加列，这里不受影响。首行若为表头则跳过。
    """

    try:
        with zipfile.ZipFile(path) as archive:
            names = [name for name in archive.namelist() if name.lower().endswith(".csv")]
            if len(names) != 1:
                raise DataError(f"{path}: expected exactly one CSV entry, found {len(names)}")
            with archive.open(names[0]) as raw:
                text = io.TextIOWrapper(raw, encoding="utf-8", newline="")
                reader = csv.reader(text)
                for index, row in enumerate(reader):
                    if not row:
                        continue
                    if index == 0 and not row[0].strip().isdigit():
                        continue
                    if len(row) < 6:
                        raise DataError(f"{path}: row {index + 1} has {len(row)} columns, need >= 6")
                    try:
                        open_time = _open_time_ms(int(row[0]))
                        open_, high, low, close, volume = (float(v) for v in row[1:6])
                    except ValueError as exc:
                        raise DataError(f"{path}: row {index + 1} is not numeric") from exc
                    if start_ms is not None and open_time < start_ms:
                        continue
                    if end_ms is not None and open_time >= end_ms:
                        continue
                    yield open_time, open_, high, low, close, volume
    except zipfile.BadZipFile as exc:
        raise DataError(f"{path}: not a valid zip archive") from exc


def _utc_day(open_time_ms: int) -> date:
    return datetime.fromtimestamp(open_time_ms / 1000.0, tz=timezone.utc).date()


def _validate_row(path: Path, row: tuple[int, float, float, float, float, float]) -> None:
    _, open_, high, low, close, volume = row
    values = (open_, high, low, close, volume)
    if not all(math.isfinite(v) for v in values):
        raise DataError(f"{path}: non-finite OHLCV at {row[0]}")
    if open_ <= 0 or high <= 0 or low <= 0 or close <= 0:
        raise DataError(f"{path}: non-positive price at {row[0]}")
    if volume < 0:
        raise DataError(f"{path}: negative volume at {row[0]}")
    if high < low:
        raise DataError(f"{path}: high < low at {row[0]}")


def load_klines(
    data_dir: str | Path,
    pair: str,
    interval: str,
    start_ms: int | None = None,
    end_ms: int | None = None,
) -> LoadedKlines:
    """读取一个交易对/周期，返回按时间升序的 OHLCV。"""

    if start_ms is not None and end_ms is not None and start_ms >= end_ms:
        raise DataError("start_ms must be < end_ms")

    monthly_dir, daily_dir = archive_dirs(data_dir, pair, interval)
    monthly = _zip_files(monthly_dir, _MONTHLY_RE)
    daily = _zip_files(daily_dir, _DAILY_RE)
    if not monthly and not daily:
        raise DataError(
            f"no Binance spot archives for {pair} {interval} under {monthly_dir.parent.parent.parent}"
        )

    daily_days = {day for day, _ in daily}
    rows: dict[int, tuple[int, float, float, float, float, float]] = {}

    def add(path: Path, iterator: Iterable[tuple[int, float, float, float, float, float]]) -> None:
        for row in iterator:
            _validate_row(path, row)
            if row[0] in rows:
                raise DataError(f"{path}: duplicate open_time {row[0]}")
            rows[row[0]] = row

    # 月归档先读，日归档覆盖到的日期整段跳过，避免同一 bar 出现两次。
    for _, path in monthly:
        add(
            path,
            (
                row
                for row in _iter_rows(path, start_ms, end_ms)
                if _utc_day(row[0]) not in daily_days
            ),
        )
    for _, path in daily:
        add(path, _iter_rows(path, start_ms, end_ms))

    if not rows:
        raise DataError(f"no rows in requested range for {pair} {interval}")

    ordered = [rows[key] for key in sorted(rows)]
    open_time = pd.Series([row[0] for row in ordered], dtype="int64")
    frame = pd.DataFrame(
        {
            "open": [row[1] for row in ordered],
            "high": [row[2] for row in ordered],
            "low": [row[3] for row in ordered],
            "close": [row[4] for row in ordered],
            "volume": [row[5] for row in ordered],
        }
    )
    frame.index = pd.to_datetime(open_time, unit="ms", utc=True)
    frame.index.name = "time"
    return LoadedKlines(
        frame=frame,
        files=len(monthly) + len(daily),
        rows=len(frame),
        first_ms=int(open_time.iloc[0]),
        last_ms=int(open_time.iloc[-1]),
    )
