"""Binance 现货归档读取。

目录布局与 `data-reader` 一致：

    {data_dir}/binance/data/spot/{daily,monthly}/klines/{symbol}/{interval}/*.zip

日归档覆盖到的 UTC 日期优先于月归档；月归档中同一天的行会被丢弃。时间戳统一
为毫秒。读取阶段只做结构校验，不修补缺口。

实现上优先用 pandas 的 C 解析器按 zip 读取整块 CSV，再做向量化校验和拼接；
逐行 Python 解析在 1m 全量（约 485 万行）上会成为回测的主要耗时。
"""

from __future__ import annotations

import io
import re
import zipfile
from dataclasses import dataclass
from datetime import date
from pathlib import Path

import numpy as np
import pandas as pd

_MONTHLY_RE = re.compile(r"-(?P<year>\d{4})-(?P<month>\d{2})\.zip$")
_DAILY_RE = re.compile(r"-(?P<year>\d{4})-(?P<month>\d{2})-(?P<day>\d{2})\.zip$")

# Binance 现货归档从 2025-01-01 起把 open_time/close_time 改为微秒；期货仍是毫秒。
# 毫秒时间戳要到公元 5138 年才会达到 1e14，用数量级即可安全区分。
_MICROSECOND_THRESHOLD = 10**14
_MS_PER_DAY = 86_400_000
_EPOCH_ORDINAL = date(1970, 1, 1).toordinal()

_COLUMNS = ["open_time", "open", "high", "low", "close", "volume"]
_DTYPES = {
    "open_time": np.int64,
    "open": np.float64,
    "high": np.float64,
    "low": np.float64,
    "close": np.float64,
    "volume": np.float64,
}


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


def _day_key(day: date) -> int:
    """UTC 日期 -> 自 1970-01-01 起的天数。"""

    return day.toordinal() - _EPOCH_ORDINAL


def _read_archive(
    path: Path,
    start_ms: int | None,
    end_ms: int | None,
) -> tuple[np.ndarray, np.ndarray] | None:
    """读取一个 zip 内唯一的 CSV，返回 `(毫秒时间戳, OHLCV 矩阵)`。"""

    try:
        with zipfile.ZipFile(path) as archive:
            names = [name for name in archive.namelist() if name.lower().endswith(".csv")]
            if len(names) != 1:
                raise DataError(f"{path}: expected exactly one CSV entry, found {len(names)}")
            raw = archive.read(names[0])
    except zipfile.BadZipFile as exc:
        raise DataError(f"{path}: not a valid zip archive") from exc

    if not raw.strip():
        return None
    first_field = raw.split(b"\n", 1)[0].split(b",", 1)[0].strip().strip(b'"')
    # Binance 归档通常无表头；仍兼容首行是列名的数据集。
    has_header = not first_field.isdigit()
    try:
        frame = pd.read_csv(
            io.BytesIO(raw),
            header=0 if has_header else None,
            usecols=[0, 1, 2, 3, 4, 5],
            names=_COLUMNS,
            dtype=_DTYPES,
        )
    except (ValueError, pd.errors.ParserError) as exc:
        raise DataError(f"{path}: not a valid kline CSV") from exc
    if frame.empty:
        return None

    stamps = frame["open_time"].to_numpy(dtype=np.int64)
    values = frame[["open", "high", "low", "close", "volume"]].to_numpy(dtype=np.float64)
    if not np.isfinite(values).all():
        raise DataError(f"{path}: non-finite OHLCV")
    if (values[:, :4] <= 0).any():
        raise DataError(f"{path}: non-positive price")
    if (values[:, 4] < 0).any():
        raise DataError(f"{path}: negative volume")
    if (values[:, 1] < values[:, 2]).any():
        raise DataError(f"{path}: high < low")

    stamps = np.where(stamps >= _MICROSECOND_THRESHOLD, stamps // 1000, stamps)
    if stamps.size > 1 and np.any(np.diff(stamps) <= 0):
        raise DataError(f"{path}: timestamps are not strictly increasing")

    keep = np.ones(stamps.size, dtype=bool)
    if start_ms is not None:
        keep &= stamps >= start_ms
    if end_ms is not None:
        keep &= stamps < end_ms
    if not keep.all():
        stamps = stamps[keep]
        values = values[keep]
    if stamps.size == 0:
        return None
    return stamps, values


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

    daily_days = [_day_key(day) for day, _ in daily]
    chunks: list[tuple[np.ndarray, np.ndarray]] = []

    # 月归档先读，日归档覆盖到的日期整段跳过，避免同一 bar 出现两次。
    for _, path in monthly:
        loaded = _read_archive(path, start_ms, end_ms)
        if loaded is None:
            continue
        stamps, values = loaded
        if daily_days:
            keep = ~np.isin(stamps // _MS_PER_DAY, daily_days)
            if not keep.all():
                stamps = stamps[keep]
                values = values[keep]
        if stamps.size:
            chunks.append((stamps, values))
    for _, path in daily:
        loaded = _read_archive(path, start_ms, end_ms)
        if loaded is not None:
            chunks.append(loaded)

    if not chunks:
        raise DataError(f"no rows in requested range for {pair} {interval}")

    stamps = np.concatenate([chunk[0] for chunk in chunks])
    values = np.concatenate([chunk[1] for chunk in chunks])
    order = np.argsort(stamps, kind="stable")
    stamps = stamps[order]
    values = values[order]
    if stamps.size > 1 and np.any(np.diff(stamps) == 0):
        raise DataError(f"duplicate open_time in archives for {pair} {interval}")

    index = pd.to_datetime(stamps, unit="ms", utc=True)
    frame = pd.DataFrame(
        values,
        index=index,
        columns=["open", "high", "low", "close", "volume"],
    )
    frame.index.name = "time"
    return LoadedKlines(
        frame=frame,
        files=len(monthly) + len(daily),
        rows=len(frame),
        first_ms=int(stamps[0]),
        last_ms=int(stamps[-1]),
    )
