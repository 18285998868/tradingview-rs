"""Binance spot backtest worker.

Rust 服务通过 `python -m backtest_worker.cli` 调用本包，stdin 不传数据，
请求与结果都走 JSON 文件。包内只依赖 numpy / pandas / vectorbt。
"""

__version__ = "0.1.0"
