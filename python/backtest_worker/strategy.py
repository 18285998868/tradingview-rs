"""双均线只做多策略。

信号口径：T 收盘比较 fast/slow 均线，目标仓位在 T+1 开盘执行。返回的
`entries`/`exits` 已经右移一根 bar，引擎只需在信号当根以开盘价成交。
"""

from __future__ import annotations

import pandas as pd

STRATEGY = "dual_ma"
STRATEGY_VERSION = "1"


def dual_ma_signals(
    frame: pd.DataFrame,
    fast: int,
    slow: int,
) -> tuple[pd.Series, pd.Series]:
    """返回与 frame 等长的 (entries, exits) 布尔序列。"""

    if fast < 1 or slow < 2 or fast >= slow:
        raise ValueError(f"invalid dual_ma params: fast={fast}, slow={slow}")
    if len(frame) < slow + 2:
        raise ValueError(
            f"insufficient bars for fast={fast}, slow={slow}: have {len(frame)}, need {slow + 2}"
        )

    close = frame["close"].astype("float64")
    fast_ma = close.rolling(fast, min_periods=fast).mean()
    slow_ma = close.rolling(slow, min_periods=slow).mean()
    target = (fast_ma > slow_ma).astype("float64").shift(1).fillna(0.0) > 0
    previous = target.shift(1, fill_value=False)
    return target & ~previous, ~target & previous
