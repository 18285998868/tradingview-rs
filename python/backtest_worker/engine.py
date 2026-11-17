"""回测引擎接口。

worker 与具体引擎之间只通过本文件的数据类交互：

- `EngineRequest`：价格、信号、成本与输出上限；
- `Engine.run()`：返回与 Rust 契约同名的字典
  （metrics/trades/trades_truncated/drawdown_events/equity/underwater）。

新增引擎时实现 `Engine` 协议并在 `get_engine` 注册即可，`cli.py` 不需要改。
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Protocol

import pandas as pd

WORKER_VERSION = "0.1.0"


class EngineError(RuntimeError):
    """引擎无法执行（依赖缺失、数据不足等）。"""


@dataclass(frozen=True)
class EngineRequest:
    """引擎无关的输入。"""

    timeframe: str
    close_price: pd.Series
    open_price: pd.Series
    entries: pd.Series
    exits: pd.Series
    fee_bps: float
    slippage_bps: float
    initial_balance: float
    max_curve_points: int
    max_trades: int


class Engine(Protocol):
    """回测引擎协议。"""

    name: str

    def version(self) -> str:
        """引擎版本，写入结果元数据。"""

    def run(self, request: EngineRequest) -> dict[str, object]:
        """执行单组参数，返回契约字典。"""


def get_engine(name: str = "vectorbt") -> Engine:
    """按名称创建引擎；当前只有 vectorbt。"""

    if name == "vectorbt":
        try:
            from .engines.vectorbt_engine import VectorBTEngine
        except ImportError as exc:
            raise EngineError(
                "vectorbt is not installed; point backtest.python_bin at the venv that has vectorbt"
            ) from exc
        return VectorBTEngine()
    raise EngineError(f"unknown engine {name!r}")
