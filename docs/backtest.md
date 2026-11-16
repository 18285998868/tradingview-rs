# 回测、回撤与对比

本文说明回测子系统的边界、接口、状态机与验证方式。架构裁决见
[`./adr/0001-rust-python-vectorbt.md`](./adr/0001-rust-python-vectorbt.md)；
分阶段实现记录见 [`../STEP.d/08-backtest.md`](../STEP.d/08-backtest.md)；
四层验证的方法、数据与偏差见 [`./validation.md`](./validation.md)。

## 范围

- 市场：Binance 现货，只做多；永续、做空、杠杆、实盘、实时行情不在本版。
- 周期：`1m/3m/5m/15m/30m/1h/2h/4h/6h/8h/12h/1d`，全部按原生归档读取。
- 策略：`dual_ma`，默认 `fast=10, slow=30`；信号在 T 收盘生成，T+1 开盘成交。
- 成本：默认手续费 10 bps、滑点 5 bps；可配置为 0。
- 引擎：vectorbt 1.2.0，由独立 Python 进程执行。
- 对比：2 到 10 个成功运行；参数扫描结果可批量勾选。

## 运行架构

Rust 服务负责 API、校验、任务状态、SQLite 持久化、对比与 CSV；Python worker
只做计算：

```text
POST /api/backtests
  -> Rust 校验 + 展开参数网格 + 写 backtest_runs(queued)
  -> spawn python -m backtest_worker.cli --request <req>.json --output <res>.json
  -> worker 读 zip、构造信号、vectorbt 计算、回撤/指标/降采样
  -> Rust 读结果、事务落库、状态置 succeeded/failed
```

- 请求/结果/日志目录默认 `<data_dir>/backtests`，按运行 ID 隔离。
- 工作目录为配置的 `worker_dir`，`PYTHONPATH` 指向同目录。
- 一次参数扫描在同一个 worker 进程内完成，共享一个批次 ID，每组参数一条运行。
- worker 的 stdout/stderr 写入日志文件；失败时保留尾部供 `/log` 读取。
- 归档时间戳统一为毫秒：Binance 现货 2025-01-01 起用微秒，worker 按数量级归一，请求的 `start_ms`/`end_ms` 也按毫秒比较。

配置示例：

```toml
[backtest]
enabled = true
python_bin = "/data/vbt/.venv/bin/python"
worker_dir = "python"
timeout_secs = 1800
max_concurrent = 2
max_compare_runs = 10
max_grid_runs = 100
max_curve_points = 2000
max_trades = 200000
keep_equity = true
```

`python_bin` 必须是装有 vectorbt 1.2.0 的解释器；系统 `python3` 通常不满足。
`enabled = false` 时所有 `/api/backtests*` 返回 503，其余行情接口不受影响。

## HTTP 接口

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/backtests/config` | 策略、周期、默认参数与上限 |
| POST | `/api/backtests` | 提交单组或参数扫描 |
| GET | `/api/backtests?limit=` | 最近运行列表 |
| GET | `/api/backtests/{id}` | 详情：指标、交易、回撤事件、曲线、买卖点 |
| DELETE | `/api/backtests/{id}` | 删除运行及明细 |
| GET | `/api/backtests/{id}/log` | 失败日志尾部 |
| POST | `/api/backtests/{id}/retry` | 用原参数创建新运行 |
| POST | `/api/backtests/compare` | 2 到 10 个运行对比 |
| GET | `/api/backtests/compare.csv?ids=a,b` | 对比指标 CSV |

提交示例：

```json
{
  "pair": "BTCUSDT",
  "timeframe": "1h",
  "params": { "fast": 10, "slow": 30 },
  "param_grid": { "fast": [5, 10], "slow": [20, 30] },
  "fee_bps": 10,
  "slippage_bps": 5,
  "initial_balance": 10000,
  "start_ms": 1609459200000,
  "end_ms": 1735689600000
}
```

`param_grid` 的两个轴做笛卡尔积；缺省轴使用 `params` 的值。超过
`max_grid_runs` 直接 400。`direction` 只接受 `long`，其他值返回可读的 400。

## 状态机与失败语义

```text
queued -> running -> succeeded
                  -> failed
```

- 服务重启时遗留的 `queued/running` 一律标记 `failed`，错误写明中断原因。
- worker 缺失、退出码非 0、超时、结果文件缺失或 JSON 非法：整批失败，日志保留。
- 参数扫描中单组数据不足或该组计算抛错：只把该组标 `failed`，其余组继续落库；已成功的运行不回滚，批次不做整体重试。
- 重试创建新运行 ID 和新批次，原运行及其历史保留。
- 删除运行会级联删除交易、回撤事件、权益与水下曲线。

## 存储与保留

`migrations/002_backtest.sql` 新增五张表：

- `backtest_runs`：元数据、状态、参数、成本、数据区间、指标 JSON；
- `backtest_trades`：逐笔交易；
- `backtest_drawdown_events`：峰值、谷底、恢复、深度、持续期；
- `backtest_equity`：降采样权益（`keep_equity=false` 时跳过）；
- `backtest_underwater`：降采样水下曲线，始终保留。

曲线按 `max_curve_points` 做保端点降采样；指标、回撤事件与交易明细按
`max_trades` 截断。`keep_equity=false` 只关闭权益曲线，回撤分析仍完整可用。

## 对比规则

- 数量少于 2 或多于 `max_compare_runs`：400，不静默截断。
- 曲线按时间戳并集对齐；不同区间缺失处写 `null`，不重采样、不填充。
- 以下元数据不一致时产生警告：标的/市场、周期、策略版本、请求/实际数据区间、成本配置。
- 参数不同不警告，但每个运行的参数与运行 ID 必须展示。
- CSV 一行一个运行，列包含参数、成本、数据区间与全部回撤指标。

## 前端

`frontend/src/backtest.ts` 提供抽屉面板：参数表单、参数扫描、运行列表轮询、
详情、权益/水下 SVG、回撤事件、交易表、最多 10 个运行的对比、图例开关、
CSV、日志、重试与删除。点击运行详情会把买卖点叠加到主图，可单独清除。

## 验证

```bash
# Rust 契约与状态机（假 worker）
cargo test --test backtest_test

# Python 数据读取、引擎、回撤、指标、CLI 与独立参考实现对账
cd python
TRADINGVIEW_REAL_DATA=/data/data /data/vbt/.venv/bin/python -m unittest discover -s tests -v

# 前端类型检查与单测
cd frontend && npm run build
cd frontend && npm test
```

`python/tests/reference.py` 是不依赖 vectorbt 的 pandas/numpy 参考实现，覆盖成本
公式、无前视、收盘权益、回撤事件、降采样与确定性；真实数据用例由
`TRADINGVIEW_REAL_DATA` 门控。完整结果与性能、浏览器、安全验证见
[`./validation.md`](./validation.md)。

真实数据冒烟需要配置 `python_bin` 与 `worker_dir`，然后提交一次单组回测与一次
参数扫描，检查详情、日志、对比、CSV、重试与删除。

## 已知限制

- 性能阈值（AC-P01/P02/P03）已在 2026-11-16 的基准机验证；结论限于单机、热页
  缓存与只读压测，不能外推到冷盘或更低配置，见 `./validation.md`。
- 首屏只加载 400 根（200 根初始 + 200 根右缘预取），默认可视约 50 根；规格要求
  500 根视窗、前后各 500 根预取，尚未对齐。
- 周期切换后视口重置到最新，未保持同一可视时间范围。
- K 线区间选择未实现；`minBarSpacing=0.5` 使可视上限约 2,800 根。
- 服务没有内置认证，默认只监听回环；开公网必须另加认证与访问控制。
- 请求体上限硬编码为 2 MiB，不可配置。
- Docker Compose 部署与数据库快照恢复未提供/未演练。
- 参数扫描使用 fork 进程池；Python 3.14 在多线程进程中 fork 会告警，未来需要
  改 `spawn` 并共享行情表。
- 首版没有策略插件机制；新增策略需要同时改 Rust 契约、Python worker 与前端表单。
- 对比视图命名保存（P1）未实现。
