"""权益/水下曲线降采样。"""

from __future__ import annotations

import math
from typing import Sequence


def select_curve_indices(
    size: int,
    max_points: int,
    events: Sequence[tuple[int, float]],
) -> list[int]:
    """均匀采样 + 首尾 + 最深若干回撤的谷底。

    `events` 为 `(trough_index, depth_pct)`，用于保证最深的回撤谷底不被
    降采样抹掉。
    """

    if size <= max_points:
        return list(range(size))
    max_points = max(10, int(max_points))
    mandatory = {0, size - 1}
    top_events = sorted(events, key=lambda event: event[1], reverse=True)
    limit = max(5, max_points // 10)
    mandatory.update(index for index, _ in top_events[:limit] if 0 <= index < size)

    stride = max(1, math.ceil(size / max_points))
    selected = set(range(0, size, stride)) | mandatory
    if len(selected) > max_points:
        removable = sorted(selected - mandatory)
        keep = max(0, max_points - len(mandatory))
        if keep == 0:
            selected = set(sorted(mandatory)[:max_points])
        else:
            step = len(removable) / keep
            picked = {removable[min(len(removable) - 1, int(i * step))] for i in range(keep)}
            selected = mandatory | picked
    return sorted(selected)
