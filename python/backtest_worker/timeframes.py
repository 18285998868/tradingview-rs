"""支持的周期与年化系数。

`PERIODS_PER_YEAR` 只用于请求校验与契约里的 `periods_per_year` 字段；
vectorbt 的年化由 `freq=timeframe` 自行计算（settings 的 year_freq 为 365 天，
与本表一致）。
"""

from __future__ import annotations

PERIODS_PER_YEAR: dict[str, float] = {
    "1m": 365 * 24 * 60,
    "3m": 365 * 24 * 20,
    "5m": 365 * 24 * 12,
    "15m": 365 * 24 * 4,
    "30m": 365 * 24 * 2,
    "1h": 365 * 24,
    "2h": 365 * 12,
    "4h": 365 * 6,
    "6h": 365 * 4,
    "8h": 365 * 3,
    "12h": 365 * 2,
    "1d": 365,
}

TIMEFRAMES: tuple[str, ...] = tuple(PERIODS_PER_YEAR)
