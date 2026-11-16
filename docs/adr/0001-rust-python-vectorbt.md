# ADR 0001：Rust 服务 + Python vectorbt worker

## 状态

已采纳（2026-11-16）。依据：冻结规格 v1.0 要求 Python/vectorbt；仓库是 Rust/axum；
用户要求 Rust 部分严格走 rust-workflow。

## 背景

规格要求后端使用 Python 与 vectorbt，但目标仓库 `tradingview-rs` 已是一个可运行的
Rust/axum 服务，包含 K 线读取、UDF、数据质量报告、布局/绘图/偏好持久化与
lightweight-charts 前端。全量改写成 Python 会丢弃这些已验证能力，也无法继续用
rust-workflow 约束主要代码。纯 Rust 回测又违反 `vectorbt` 的明确要求。

## 决策

保留 Rust 服务作为 API、数据、任务状态、图表与持久化层；新增一个独立的 Python
worker 作为回测计算层，使用 vectorbt 1.2.0。

### 边界

Rust 负责：

- `/api/backtests` 系列 HTTP 路由、参数校验、任务状态机；
- SQLite 持久化：运行元数据、指标、交易、回撤事件、降采样曲线；
- 调用 Python 子进程、超时、失败日志、重试；
- 对比上限、元数据一致性警告、CSV 导出；
- 前端静态资源与现有行情 API。

Python 负责：

- 读取 Binance 现货归档 zip（`{data_dir}/binance/data/spot/...`）；
- 构造双均线只做多信号，T 收盘生成、T+1 开盘成交；
- 用 vectorbt `Portfolio.from_orders` 计算权益与交易；
- 计算回撤事件、汇总指标、降采样曲线；
- 一次进程内完成参数扫描，输出单份 JSON。

### 交互

Rust 把请求写成 JSON 文件，执行：

```text
{python_bin} -m backtest_worker.cli --request <request.json> --output <result.json>
```

工作目录为 `python/`，环境变量 `PYTHONPATH` 指向同一目录。worker 把结果写入
`result.json`，进度写 stdout，错误写 stderr。Rust 只信任自己生成的文件路径。

请求与结果的 JSON 形状由 `src/app/backtest/model.rs` 与
`python/backtest_worker/cli.py` 共同约束；任一侧字段变更必须同时更新两侧测试。

### 失败模式

- Python 缺失、退出码非 0、超时、结果文件缺失或 JSON 非法：整批运行标记
  `failed`，保存 stderr 尾部到日志文件，运行记录保留，可重试。
- 服务重启时仍为 `queued`/`running` 的运行标记为 `failed`，错误写明重启中断；
  已完成的运行与曲线不受影响。
- 参数扫描中单个参数组失败（数据不足、worker 为该组返回 error）：只把该组标记
  `failed`，其余组照常落库；worker 进程级失败（退出码非 0、超时、结果文件缺失或
  JSON 非法）才整批失败，避免同一批次留下无法解释的混合状态。
- 对比数量少于 2 或多于配置上限：400，不静默截断。

## 后果

正面：

- 满足规格的 vectorbt 要求，同时保留已验证的 Rust 基础设施；
- Python 可以独立测试、独立升级 vectorbt；
- 计算失败不会污染 API 进程。

负面：

- 多一个运行时依赖（Python 环境与 vectorbt）；
- 跨语言契约需要双份测试；
- 子进程启动与 JSON 序列化有额外开销，参数扫描需在一次进程内完成以摊销。

## 未采纳方案

- 全量 Python 重写：丢弃现有 Rust 能力，rust-workflow 不再适用。
- 纯 Rust 用 vta-bt：违反规格明确的 vectorbt 要求。
- Rust 内嵌 Python（PyO3）：部署与崩溃隔离更复杂，首版不引入。
