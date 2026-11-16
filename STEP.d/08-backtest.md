# 回测、回撤与对比（阶段四）

## 目标

在现有 Rust/axum 服务上增加 Binance 现货回测能力：Python vectorbt worker 计算，
Rust 负责任务、持久化、API 与前端。首版只做现货只做多。

## 范围

- 12 个原生周期：`1m`、`3m`、`5m`、`15m`、`30m`、`1h`、`2h`、`4h`、`6h`、`8h`、`12h`、`1d`。
- 策略：双均线只做多（`dual_ma`），参数 `fast`、`slow`，支持参数扫描。
- 成交口径：T 收盘生成目标仓位，T+1 开盘成交；手续费与滑点按 bps 配置。
- 结果：汇总指标、交易明细、回撤事件、降采样权益/水下曲线。
- 对比：2–10 个运行；元数据不一致警告；CSV 导出。
- 任务：状态查询、失败日志、重试。

不做：永续、做空、杠杆、实时行情、实盘、归因、多租户。

## 数据流

```text
POST /api/backtests
  -> Rust 校验 + 展开参数网格 + 写 backtest_runs(queued)
  -> tokio 任务获取并发许可
  -> 写 request.json
  -> python -m backtest_worker.cli
       -> 读 zip -> DataFrame -> vectorbt -> metrics/events/curves
  -> result.json
  -> Rust 事务写入 metrics/trades/events/curves，状态 succeeded
```

## 存储

新增 `migrations/002_backtest.sql`：

- `backtest_runs`：运行元数据、状态、参数、成本、数据区间、指标 JSON；
- `backtest_trades`：逐笔交易；
- `backtest_drawdown_events`：峰值/谷底/恢复/深度/持续期；
- `backtest_equity`：降采样权益（可配置关闭）；
- `backtest_underwater`：降采样水下曲线。

原始全量净值不默认保存；默认保存降采样曲线与全量事件/指标。`keep_equity = false`
时不写权益表，仍保留水下曲线。

## 回撤口径

- 每根 bar 用收盘 mark-to-market 权益；
- 回撤 = 当前权益 / 历史峰值 − 1；
- 峰值到谷底再到恢复为一个事件；未恢复事件 `recovered = false`；
- 深度用百分比正数（`depth_pct`），水下曲线用负数百分比；
- 零回撤序列事件数为 0，最大回撤为 0。

## 验收映射

- AC-B01/B02/B03：worker 引擎测试与固定序列对账；
- AC-B04：`direction != "long"` 直接 400；
- AC-B05：参数网格展开与参数映射测试；
- AC-B06/B07：指标/回撤单元测试与重复运行比对；
- AC-R01/R02：回撤事件与指标参考序列测试；
- AC-R03：SQLite 持久化与重启恢复测试；
- AC-R04：降采样点均来自原始点，抽样误差为 0；
- AC-R05：`keep_equity` 保留策略与级联删除测试；
- AC-C01–C06：对比 API、警告规则、10 个上限与 CSV 导出测试；
- AC-P06：失败日志与重试接口。

† 阈值（AC-D06、AC-P01/P02、AC-C01 等）尚未在基准数据上校准，当前只作为目标，
不得在汇报中写成已验证。

## 失败与重试

- worker 非 0、超时、结果非法：整批 `failed`，日志落盘；
- 服务重启：遗留 `queued`/`running` 标记 `failed`；
- 重试创建新运行，不复用旧 ID，保留历史。

## 测试计划

- Python：数据读取（日/月归档覆盖）、回撤、指标、引擎无前视、CLI 端到端；
- Rust：迁移、运行状态机、参数网格、对比警告、上限、CSV、失败恢复；
- 前端：`tsc` + 现有 node 测试；手工浏览器冒烟（如环境允许）。
