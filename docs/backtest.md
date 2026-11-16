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
| GET | `/api/backtests/comparisons` | 命名保存的对比列表（含 `missing_runs`） |
| POST | `/api/backtests/comparisons` | 保存命名对比（名称 1–64 字符，2–上限个成功运行） |
| DELETE | `/api/backtests/comparisons/{id}` | 删除命名对比 |

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

`migrations/003_comparisons.sql` 增加一张表：

- `backtest_comparisons`：命名对比的名称、运行 ID JSON 数组与时间戳；
  名称唯一；运行删除后对比保留，读取时按当前运行表统计 `missing_runs`。

曲线按 `max_curve_points` 做保端点降采样；指标、回撤事件与交易明细按
`max_trades` 截断。`keep_equity=false` 只关闭权益曲线，回撤分析仍完整可用。

## 对比规则

- 数量少于 2 或多于 `max_compare_runs`：400，不静默截断。
- 曲线按时间戳并集对齐；不同区间缺失处写 `null`，不重采样、不填充。
- 以下元数据不一致时产生警告：标的/市场、周期、策略版本、请求/实际数据区间、成本配置。
- 参数不同不警告，但每个运行的参数与运行 ID 必须展示。
- CSV 一行一个运行，列包含参数、成本、数据区间与全部回撤指标。
- 命名对比：名称 1–64 字符且唯一；只接受 2 至 `max_compare_runs` 个已成功运行；
  找回时按保存顺序恢复勾选，运行缺失只提示 `missing_runs`，不会静默补位。

## 前端

`frontend/src/backtest.ts` 提供抽屉面板：参数表单、参数扫描、运行列表轮询、
详情、权益/水下 SVG、回撤事件、交易表、最多 10 个运行的对比、图例开关、
命名对比的保存/找回/删除、CSV、日志、重试与删除。点击运行详情会把买卖点叠加
到主图，可单独清除。

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
- 首屏默认视窗 500 根，前后各预取 500 根；只保留一段连续窗口，不做无限滚动。
- 周期切换会保持同一可视时间区间（单页 10k 根、总上限 100k 根）；目标周期在该
  区间没有数据时收敛到已有边界。3d/1w/1mo 的 bar 跨度已接近或超过常用视窗。
- 区间选择用绘图库的 `date-range` 工具实现，统计栏数据来自已加载窗口；窗口外
  的区间不会额外取数。
- 服务默认只监听回环；`auth_token` 启用后 `/api/*` 与 `/udf/*` 需要 Bearer 或
  Basic。未内置 TLS，公网部署需反向代理终止 TLS。
- 请求体上限由 `max_body_bytes` 配置，默认 2 MiB，合法范围 1 KiB 至 64 MiB。
- Docker Compose 部署未提供，浏览器兼容只验证了 Firefox 160。
- 参数扫描使用 fork 进程池；Python 3.14 在多线程进程中 fork 会告警，未来需要
  改 `spawn` 并共享行情表。
- 首版没有策略插件机制；新增策略需要同时改 Rust 契约、Python worker 与前端表单。
- 命名对比不跟随运行删除级联清理，运行缺失只体现在 `missing_runs`。
