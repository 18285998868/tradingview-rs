"""回撤事件与水下曲线。

口径（与 SPEC 4.4 一致）：

- 每根 bar 收盘的 mark-to-market 权益；
- 回撤 = 当前权益 / 历史峰值 - 1；
- 从峰值到谷底再到恢复为一个事件，未恢复事件 `recovered=false`；
- 事件深度用百分比正数，水下曲线用百分比负数。
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Sequence

import numpy as np

_EPS = 1e-12


@dataclass(frozen=True)
class DrawdownEvent:
    """一次回撤事件。深度单位为百分比（正数）。"""

    peak_time: int
    trough_time: int
    recovery_time: int | None
    depth_pct: float
    duration_ms: int
    recovery_ms: int | None
    recovered: bool
    peak_index: int
    trough_index: int

    def to_dict(self, seq: int) -> dict[str, object]:
        return {
            "seq": seq,
            "peak_time": self.peak_time,
            "trough_time": self.trough_time,
            "recovery_time": self.recovery_time,
            "depth_pct": self.depth_pct,
            "duration_ms": self.duration_ms,
            "recovery_ms": self.recovery_ms,
            "recovered": self.recovered,
        }


@dataclass(frozen=True)
class DrawdownResult:
    """回撤计算结果。"""

    events: list[DrawdownEvent]
    underwater_pct: np.ndarray
    drawdown_time_pct: float

    @property
    def max_drawdown_pct(self) -> float:
        return max((event.depth_pct for event in self.events), default=0.0)

    @property
    def avg_drawdown_pct(self) -> float:
        if not self.events:
            return 0.0
        return sum(event.depth_pct for event in self.events) / len(self.events)

    @property
    def longest_drawdown_ms(self) -> int:
        return max((event.duration_ms for event in self.events), default=0)

    @property
    def longest_recovery_ms(self) -> int:
        return max((event.recovery_ms or 0 for event in self.events if event.recovered), default=0)


def compute_drawdown(times_ms: Sequence[int], equity: Sequence[float]) -> DrawdownResult:
    """计算水下曲线与事件。

    输入必须等长、时间升序、权益为有限值。零回撤序列返回空事件。
    """

    times = np.asarray(times_ms, dtype=np.int64)
    values = np.asarray(equity, dtype=np.float64)
    if times.shape != values.shape:
        raise ValueError("times_ms and equity must have the same length")
    if values.size == 0:
        raise ValueError("equity must not be empty")
    if not np.isfinite(values).all():
        raise ValueError("equity contains non-finite values")
    if np.any(values <= 0):
        raise ValueError("equity must be positive")
    if np.any(np.diff(times) <= 0):
        raise ValueError("times_ms must be strictly increasing")

    peaks = np.maximum.accumulate(values)
    underwater = (values / peaks - 1.0) * 100.0
    in_drawdown = values < peaks

    # 每个位置对应的最后一个峰值下标：值等于 running peak 的位置刷新下标，
    # 再前缀取最大。这样事件起点能回指真正的峰值 bar。
    indices = np.arange(values.size, dtype=np.int64)
    peak_indices = np.maximum.accumulate(np.where(values >= peaks, indices, 0))

    events: list[DrawdownEvent] = []
    n = values.size
    i = 0
    while i < n:
        if not in_drawdown[i]:
            i += 1
            continue
        start = i
        while i + 1 < n and in_drawdown[i + 1]:
            i += 1
        end = i
        peak_index = int(peak_indices[start])
        trough_index = int(start + np.argmin(values[start : end + 1]))
        peak_value = float(values[peak_index])
        trough_value = float(values[trough_index])
        recovered = end + 1 < n
        recovery_index = end + 1 if recovered else None
        last_time = int(times[end])
        recovery_time = int(times[recovery_index]) if recovery_index is not None else None
        events.append(
            DrawdownEvent(
                peak_time=int(times[peak_index]),
                trough_time=int(times[trough_index]),
                recovery_time=recovery_time,
                depth_pct=(peak_value - trough_value) / peak_value * 100.0,
                duration_ms=(recovery_time if recovery_time is not None else last_time) - int(times[peak_index]),
                recovery_ms=(
                    recovery_time - int(times[trough_index]) if recovery_time is not None else None
                ),
                recovered=recovered,
                peak_index=peak_index,
                trough_index=trough_index,
            )
        )
        i += 1

    drawdown_time_pct = float(in_drawdown.mean() * 100.0) if n else 0.0
    return DrawdownResult(
        events=events,
        underwater_pct=underwater,
        drawdown_time_pct=drawdown_time_pct,
    )
