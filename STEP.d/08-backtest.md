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

- AC-B01/B02/B03：独立参考实现与固定序列对账，成本手算，无前视用例；
- AC-B04：`direction != "long"` 直接 400；
- AC-B05：参数网格展开与参数映射测试，100 组真实数据扫描；
- AC-B06/B07：指标/回撤单元测试与 3 次运行哈希比对；
- AC-R01/R02：回撤事件与指标参考序列测试；
- AC-R03：SQLite 持久化与重启恢复测试；
- AC-R04：降采样点均来自原始点，抽样误差为 0；
- AC-R05：`keep_equity` 保留策略与级联删除测试；
- AC-C01–C07：对比 API、警告规则、10 个上限、CSV 与命名保存/找回测试；
- AC-P01/P02/P03/P04：release 基准、100 组扫描、600 秒并发压测与资源峰值告警；
- AC-P05：Docker Compose 单命令启动、鉴权、健康检查与状态卷持久化；
- AC-P06：失败日志与重试接口；
- AC-P07：在线快照、临时实例、覆盖恢复与抽样比对；
- AC-V05：26 个指标对接口 JSON 快照的逐点回归；
- AC-V07：Firefox 160/159 与 Chromium 157 在 1366×768/1920×1080 的布局检查。

## 验证结果

四层验证的方法与逐项数据见 `../docs/validation.md`，摘要：

- 正确性：35 项 Python unittest 全绿，含独立 pandas/numpy 参考实现；合成随机
  游走、10 组固定净值、成本手算、无前视、降采样、3 次哈希与真实 BTCUSDT 1h
  对账全部一致。
- 性能：1m 全量 4,852,639 根单组中位 13.38 秒（≤30 秒）；100 组扫描 API 端到端
  164.74 秒（≤300 秒）；5 并发 600 秒 366,612 次请求、0 错误、整体 P95 22.9 毫秒
  （≤1 秒）；压力回测进程树峰值 RSS 1.587 GiB（≤16 GiB）、磁盘 37.35 GiB
  （≤500 GiB），超限告警脚本实测退出码 2。
- 浏览器：10 运行对比 P95 94 毫秒（≤3 秒）；首屏冷 268 毫秒、热 258 毫秒
  （默认 500 可视、首屏 1500 根）；14 次同区间切换中位 93 毫秒、最大
  156 毫秒；9,939 根 1m 平移 60.34 fps，区间统计与后端逐项一致；命名对比
  在保存、服务重启、页面找回后仍渲染 2 条曲线；26 个指标 16,800 个数值点与
  `/api/klines` 快照逐点一致。
- 部署：三阶段镜像 1.31 GB；清空容器与状态卷后单条
  `docker compose up -d --build --wait` 6.3 秒进入 healthy；2 个真实回测在
  `restart` 与 `down`/`up` 后详情哈希和 9 张表行数完全一致。
- 浏览器兼容：Firefox 160/159、Chromium 157 在 1366×768 与 1920×1080 的六组
  检查全部通过，0 横向溢出；工具栏、图表与 460 px 回测抽屉均在视口内。
- 安全与运维：`cargo audit --no-fetch` 0 漏洞；23 个非法输入用例全部 4xx；
  默认仅回环，非回环未配 `auth_token` 时启动失败；Bearer/Basic 与可配置请求体
  上限实测生效；快照恢复后 9 张表行数一致、100 条行情与 10 个回测详情一致。

关键修正：旧引擎用 `from_orders(targetpercent)` 把开盘价同时当作成交价与估值价，
权益在开盘 mark；现改为 `from_signals(close, price=open)` 并按收盘重建权益。
参数扫描改用 fork 进程池（默认最多 4 个），数据读取改为 pandas 向量化。

未覆盖与偏差：AC-D01/D02/D06 数据导入项不在本轮；Docker 演练限单机 x86_64，
浏览器矩阵限桌面视口。逐项见 `../docs/validation.md` 的“未覆盖与偏差”。

## 失败与重试

- worker 非 0、超时、结果非法：整批 `failed`，日志落盘；
- 服务重启：遗留 `queued`/`running` 标记 `failed`；
- 重试创建新运行，不复用旧 ID，保留历史。

## 测试计划

- Python：数据读取（日/月归档覆盖）、回撤、指标、引擎无前视、CLI 端到端；
- Rust：迁移、运行状态机、参数网格、对比警告、上限、CSV、失败恢复；
- 前端：`tsc` + 现有 node 测试；
- 浏览器、性能、部署与安全：已按 `../docs/validation.md` 的四层计划执行；
  剩余数据导入项与单机/桌面边界同文列出。
