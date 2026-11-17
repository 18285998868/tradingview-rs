# 回测子系统验证报告

2026-11-16 在基准机上做了五层验证：数据对账、回测正确性、性能、浏览器指标、
工具链与运维安全。2026-11-17 回测引擎改为 vectorbt 原生指标后，复测了第一层
用例与第二层 AC-P01/P02；第三、四层不依赖回测数值口径，保留原结果。本文只
记录实际运行过的用例；未运行项与规格偏差在文末单列，不能把本文当作全部验收
项的签字确认。

## 结论

| 层 | 验收项 | 结果 | 关键数据 |
|---|---|---|---|
| 第零层 数据 | AC-D01/D02/D03/D04/D05/D06/D07 | 通过 | 3 标的 × 12 周期 × 1000 根 OHLC 零差异；149,048 行逐字段零差异；重复导入零重写；异常注入计数与样例齐全；110 个跨周期窗口 OHLC 零差异；100 标的 × 12 周期全量导入 14.4 分钟、10.82 GB、0 失败；一年 1m 查询 P95 362 毫秒 |
| 第一层 正确性 | AC-B01/B02/B03/B04/B05/B06/B07、AC-R01/R02/R03/R04/R05 | 通过 | 24 项 unittest 全通过；交易、回撤事件与指标改由 vectorbt 直接产出，权益按订单记录重建，并与重构前实现逐字段对账；100 组扫描参数映射精确；重启后运行与对比可查；keep_equity=false 时权益表 0 行、重试与删除后无孤儿 |
| 第二层 性能 | AC-P01/P02/P03/P04 | 通过 | 1m 全量单组中位 13.23 秒；100 组 API 端到端 163.28 秒；5 并发 P95 22.9 毫秒、0 错误；压力回测进程树峰值 RSS 1.47 GiB、磁盘 37.35 GiB |
| 第三层 浏览器 | AC-C01/C02/C03/C04/C05/C06/V01/V02/V03/V04/V06/C07/V05/V07 | 通过 | 对比 P95 94 毫秒；100 组扫描勾选 10 组的指标与详情一致、CSV 10 行一致；并集对齐缺失点写 null、5 类元数据警告精确；12 周期可选并完成同区间切换；首屏冷 268 毫秒、热 258 毫秒；同区间切换中位 93 毫秒；9,939 根 60.34 fps；1m 边缘批次 4 个请求、41,257 根无长任务；命名对比跨重启找回；26 个指标 16,800 点与接口快照一致；三浏览器六组合布局检查全过 |
| 第四层 工具链、安全与运维 | AC-S01/S02/P04/P05/P06/P07 | 通过 | 依赖审计 0 漏洞；Bearer/Basic 与可配置上限生效；23 个非法用例 4xx；失败日志可读且重试生成新批次；Compose 单命令启动 6.3 秒，重启/重建后 2 个运行与 9 张表一致；快照恢复后 9 张表行数一致、10 个回测详情一致 |

浏览器兼容覆盖 Firefox 最新两个大版本与 Chromium 当前大版本；Docker Compose
在第四层补齐。逐项状态见“未覆盖与偏差”。

## 环境与口径

- 机器：CachyOS，Intel i5-12400F（6 核 12 线程），32 GB 内存，数据盘为 NVMe ext4。
- 服务：`cargo build --release`，单进程，验证配置 `max_concurrent=1`、
  `max_grid_runs=100`、`max_curve_points=2000`，SQLite 状态库放在 `/tmp`。
- Python：3.14.10，vectorbt 1.2.0，pandas/numpy 为其依赖版本。
- 浏览器：Firefox 160.0（系统包）、Firefox 159.0（Mozilla 官方 linux-x86_64
  便携包）、Chromium 157.0.8103.78；驱动 geckodriver 0.37.1 与 ChromeDriver
  157.0.8103.78。性能项仍用 Firefox 160 的 1366×854 窗口（inner
  1366×768）；兼容矩阵使用精确的 1366×768 与 1920×1080 视口。
- 容器：Docker 29.9.3 + Compose 5.7.1 + buildx 0.39.0，镜像
  `tradingview-rs:local` 1.31 GB；容器内 uid 10001，只读根文件系统。
- 数据：Binance 现货 BTCUSDT，1m 全量 4,852,639 根（2017-08-17 至 2026-11-14，
  130 个归档），1h 原生 54,341 根；AC-V05 另取 U 本位永续 BTCUSDT 1h 400 根。
- 数据层：AC-D01 用 BTCUSDT/ETHUSDT/SOLUSDT 月档，AC-D02/D05/D07 用 BTCUSDT
  月档；AC-D03 用 BTCUSDT/ETHUSDT 2024-01 至 2024-03 的 12 个归档；AC-D04 用
  脚本生成的异常归档；AC-D06 按 24 小时成交额取前 100 个 USDT 现货标的，下载
  12 周期月档到空目录。下载器并发 64，`data.binance.vision` 直连，起始日解析
  所需的 `api.binance.com` 走本机 HTTP 代理。读取器 `binance-core 0.2.0`
  （data-reader `a844ea0`），下载器 data-downloader `f852202`。
- 计时：性能项取 10 次或 5 次的 P95/中位数；浏览器耗时取自页面内的
  `performance.mark/measure`，在 `setData`、应用首屏视窗并等两帧后记录。
- 缓存：性能与浏览器测试都在同一台机器上连续运行，操作系统的页缓存是热的；
  “冷”只表示服务进程的 DataLoader 缓存为空，见各节说明。

## 第零层 数据对账

### 方法

七个脚本只读归档，结果写 JSON，失败退出码 1；字段映射、时间口径与质量标记见
[`./data.md`](./data.md)。

- AC-D01：12 个周期各取 1000 根固定窗口，起点 2024-01-01 UTC。本地走回测
  worker 的 `load_klines`（pandas 解析、微秒归一），官方走现货
  `/api/v3/klines` 同一窗口；判定时间戳逐根相等、OHLC 逐值完全相等、
  成交量相对误差 ≤ 0.1%。
- AC-D02：12 个周期 × 2024-01/2025-01 两个月档，`binance-core` 解析结果与 zip
  内 CSV 原文逐行逐字段比较 12 列，另校验周期对齐、首行等于 UTC 月首与
  `close_time = open_time + 周期 - 1`。2025-01 现货归档是微秒时间戳，用于
  验证归一逻辑。
- AC-D03：在空目录用同一个配置连续下载两次（BTCUSDT/ETHUSDT × 1h/1d ×
  2024-01 至 2024-03），比对 zip 集合、字节数、sha256 与修改时间；再用
  `binance-core` 读取同一区间，核对两次行数一致，并检查 `last-run.json` 的
  `finished_at` 是否更新。
- AC-D04：脚本生成一个月度 1h 归档，注入 2 根缺失 bar、1 行 `high < low`、
  1 行负成交量和 1 行重复时间戳；`bad_rows="skip"` 核对 `rows_skipped`、
  `gaps`、`skipped_samples` 与派生缺失根数，`bad_rows="fail"` 与重复时间戳
  核对应以非零退出并带可读错误。
- AC-D05：BTCUSDT 2024 年 1m 全量，均匀抽 100 根 1h 与 10 根 1d；每根 1h
  必须覆盖 60 根 1m、1d 覆盖 1,440 根，比较聚合 OHLC 与成交量；同时统计
  1m/1h/1d 归档在区间内的缺失根数。
- AC-D06：按 24 小时成交额取前 100 个 USDT 现货标的（排除杠杆代币与稳定币对），
  下载 12 个周期月档到空目录；起始日由下载器按各标的归档首日解析，默认
  sha256 校验，`data.binance.vision` 直连、`api.binance.com` 走本机代理，
  并发 64。计时包含起始日解析与全部下载，存储为全部 zip 字节（校验后不保留
  sidecar）。
- AC-D07：按标的与 `[start, end)` 读取一年 1m，先记录一次冷启动，再预热后
  计时 100 次取 P95；查询路径复用回测 worker 的 `load_klines`，归档在文件
  选择阶段按区间裁剪。

### AC-D01 官方 K 线对账

| 标的 | 周期数 | 对照根数 | OHLC 差异 | 成交量最大相对误差 |
|---|---|---|---|---|
| BTCUSDT | 12 | 12,000 | 0 | 0 |
| ETHUSDT | 12 | 12,000 | 0 | 0 |
| SOLUSDT | 12 | 12,000 | 0 | 0 |

三个标的共 36,000 根全部落在同一窗口（2024-01-01 起），逐根时间戳、OHLC 与
成交量都与官方一致。

### AC-D02 源文件逐字段对账

- 24 个文件、149,048 行，12 列逐字段差异 0，缺口 0。
- 2025-01 的微秒归一覆盖 12 个周期；每行 `close_time` 与 `open_time + 周期 - 1`
  完全匹配，首行都等于对应 UTC 月首。
- 抽样要求 1000 行，实际做了整月全量比对，样本是其超集。

### AC-D03 重复导入

在空目录连续执行两次下载，固定配置为 2 标的 × 2 周期 × 3 个月：

| 指标 | 第一次 | 第二次 |
|---|---|---|
| 退出码 | 0 | 0 |
| 文件数 | 12 | 12 |
| 字节数 | 263,024 | 263,024 |
| 新增 / 删除 / 重写 | - | 0 / 0 / 0 |
| sha256 变化 | - | 0 |
| BTC/ETH 1h 行数 | 2,184 / 2,184 | 2,184 / 2,184 |
| BTC/ETH 1d 行数 | 91 / 91 | 91 / 91 |
| `last-run.json` 的 `finished_at` | 13:34:02Z | 13:34:34Z |

第二次运行耗时 32.2 秒，主要花在远端清单核对，totals 为 0，未重写任何文件。
`last-run.json` 每次结束都原子更新，更新时间可查。

### AC-D04 异常注入

| 注入异常 | 数量 | 观测 |
|---|---|---|
| 缺失 bar | 2 | `gaps=1`，按首尾推算缺失 2 根 |
| `high < low` | 1 | `rows_skipped` 计入并保留样例 |
| 负成交量 | 1 | `rows_skipped` 计入并保留样例 |
| 重复 `open_time` | 1 | `skip` 也以非零退出，错误含 `duplicate or misordered` |

`bad_rows="skip"` 时读取结果为 8 行（10 根正常 bar 去掉 2 根），
`rows_skipped=2`、`skipped_samples` 两条分别指出 `invalid OHLC` 与
`negative value`；`bad_rows="fail"` 时首行坏数据即退出 1，错误带文件名与
CSV 行号。重复时间戳在合并阶段被拒绝，不静默去重。

### AC-D05 跨周期一致性

BTCUSDT 2024 年 1m 归档共 527,040 行，按首尾推算缺失 0 根。

| 对比 | 抽样 | 每窗 1m 根数 | OHLC 差异 | 成交量最大相对误差 |
|---|---|---|---|---|
| 1h | 100 | 60 | 0 | 3.64e-16 |
| 1d | 10 | 1,440 | 0 | 1.34e-16 |

110 个抽样窗口全部覆盖完整，聚合 OHLC 与原生逐值相等，成交量误差远低于
0.1% 阈值。

### AC-D06 全量导入

| 指标 | 实测 |
|---|---|
| 标的 × 周期 | 100 × 12 个周期（月度归档） |
| 时间 | 864 秒（14.4 分钟），含起始日解析 |
| 文件 | 49,620 个 zip，0 个 404、0 个失败 |
| 存储 | 10.82 GB（10.07 GiB），平均 103 MiB/标的 |
| 每标的文件数 | 最少 12、中位 372、最多 1,332 |
| 完整性 | `binance-data-verify` 复核 49,620 个归档全部可读 |
| 吞吐 | 57.4 个文件/秒、12.5 MB/秒 |

固定数据集是“按成交额前 100 个现货标的 × 各自完整归档历史 × 12 周期”，新上市
标的只有数月，最多 9 年（BTC/ETH）。若 100 个标的全都有 9 年历史，按文件数与
BTC 单标的体积上界估算为 133,200 个文件、约 34 GB、约 39 分钟；本次实测值低于
该上界。

AC-D06 的 † 阈值按上界加余量冻结：全量导入 ≤ 60 分钟、存储 ≤ 50 GB。换网络、
并发或加入日档/其他数据集需要重测。

### AC-D07 一年 1m 查询

BTCUSDT 2024-01-01 至 2025-01-01，共 527,040 根 1m，读取 12 个月档：

| 指标 | 实测 |
|---|---|
| 冷启动 | 357 ms |
| P50 | 322 ms |
| P95 | 362 ms |
| P99 | 407 ms |
| 最大 | 436 ms |
| 阈值 | 2,000 ms |

首次实现会读取 2017 年以来的全部 130 个归档后再按时间过滤，P95 为 2,995 ms。
现在 `load_klines` 在扫描文件时先按 `[start, end)` 裁剪归档，一年查询只读
12 个月档；新增 3 项单测覆盖月档、日档与无交集区间。计时在页缓存热的同一台
机器上取得，清页缓存后的冷读会更高。

### 边界

- AC-D01 只覆盖三个主流标的与 2024-01-01 起窗口；官方 REST 与归档同源，
  能发现本地解析与归一引入的差异，不能发现上游系统性口径错误。
- AC-D02 抽查两个月档而非全库扫描；覆盖全部 12 列字段。
- AC-D03 只覆盖 12 个文件的小数据集，未覆盖日档、断点续传与并发写入。
- AC-D04 是脚本生成的合成异常，不代表生产归档的真实损坏分布。
- AC-D05 只覆盖 BTCUSDT 2024 年，未覆盖其他标的与更长区间。
- AC-D06 不含日档与 `aggTrades`/`trades` 等数据集；“9 年”是 BTC/ETH 的完整
  历史，新标的不足 9 年；耗时为单机单次结果，归档域名直连。
- AC-D07 的路径是回测 worker 的 `load_klines`，不是 HTTP 查询接口；计时在
  页缓存热的同一台机器上取得。
- 100 标的按 2026-11-16 的 24 小时成交额快照选取，标的池随市场变化，重跑会
  得到不同的名单。

## 第一层 正确性对账

### 方法

2026-11-17 起回测指标、回撤与水下曲线全部由 vectorbt accessor 产出，不再维护
独立的 pandas/numpy 参考实现。对账改为两层：

- 适配器契约测试：`python/tests/` 的 24 项 unittest 覆盖数据读取、双均线无前视、
  成本与回撤事件字段、曲线截断、降采样、确定性与 CLI；
- 重构前后逐字段对账：同一份 BTCUSDT 1h 2024H1 数据分别跑旧实现与新适配器，
  比较交易、回撤事件与汇总指标；1m 全量另核对订单重建权益与 `portfolio.value()`
  的逐 bar 差异。

对账口径在实现前先冻结：

- 信号在 T 收盘比较 fast/slow 均线，目标仓位右移一根 bar，在 T+1 开盘成交；
- 入场数量为 `现金 / (1 + 手续费率) / 买价`，买价为 `open × (1 + 滑点率)`；
- 卖出价为 `open × (1 - 滑点率)`，卖出后现金为成交额减手续费；
- 每根 bar 的权益按收盘价 mark-to-market，未平仓头寸按最后一根收盘价估值；
- 回撤按收盘权益计算，峰值到谷底再到恢复为一个事件。

### 用例与结果

| 用例 | 内容 | 结果 |
|---|---|---|
| 适配器契约 | 无前视、成本、回撤事件字段、曲线截断、确定性、金值 | 7 项 `test_engine` 全通过 |
| 降采样 | 短序列、端点、最深谷底、越界事件 | 4 项 `test_curve` 全通过 |
| 数据读取 | 月档/日档、区间裁剪、微秒时间戳、坏 zip | 9 项 `test_data` 全通过 |
| CLI 契约 | 非现货、重复参数、输出形状、单组失败隔离 | 4 项 `test_cli` 全通过 |
| 重构前后对账 | BTCUSDT 1h 2024H1，3,648 根 | 78 笔交易、15 个回撤事件与旧实现一致；除 Sortino 外的指标差 <1e-12（Sortino 改用 vectorbt 口径，-0.467 → -0.843） |
| 1m 权益特化 | BTCUSDT 1m 全量 4,852,639 根 | 订单重建权益与 `portfolio.value()` max abs diff 0.0 |
| 1m 全量确定性 | 同输入连续 5 次 | 6,784 笔交易、4 个回撤事件、指标与终值完全一致 |

执行结果：`Ran 24 tests`，`OK`，退出码 0。

### 发现并修复的缺陷

1. 权益按开盘价估值。旧实现用 `from_orders(targetpercent)` 只接收一个价格序列，
   成交价与估值价都是开盘价；实际权益应在收盘 mark。修复为
   `from_signals(close=收盘价, price=开盘价)`，再按订单记录逐 bar 重建收盘权益。
   回归用例为 `test_equity_is_marked_to_close_not_open`。
2. 交易时间戳逐个调用 `Timestamp.timestamp()`，在百万级索引上开销明显；改为
   索引直接转毫秒整数。该改动只影响性能，不改变数值。
3. 逐行解析 1m 归档约 485 万行，是单组回测的主要耗时；改为 pandas C 解析器
   整块读取后再向量化校验、过滤、拼接。
4. 回测指标与回撤曾在 vectorbt 外重写一套（`metrics.py` / `drawdown.py`），
   等于维护两套口径；现改为 vectorbt 原生 accessor，适配器只做字段映射。
   `portfolio.value()` 在 1m 全量上需 19–28 秒，保留订单记录重建权益作为唯一
   特化，避免 AC-P01 超时。
5. Rust 反序列化 worker 结果时浮点默认走 serde_json 的快速解析路径，落库指标
   可能与 `result.json` 相差 1 ulp；`serde_json` 启用 `float_roundtrip` 后逐位
   一致（100 组扫描复核 0 差异）。

### AC-R05 保留策略与孤儿数据

- 默认 `keep_equity = true`，保存降采样权益与水下曲线；Rust 集成测试断言详情
  返回 2 个权益点与 3 个水下点。默认值在 `BacktestConfig::default` 冻结。
- 演练 `scripts/retention_check.py` 用 `keep_equity = false` 启动临时服务：
  BTCUSDT 1h 2024 年 Q1（2,184 根）单组回测成功，`retention = underwater_only`；
  详情 `equity = []`，`backtest_equity` 0 行；水下曲线 1,098 点、15 个回撤事件、
  49 笔交易与全部指标仍在。
- 重试生成新运行 ID，旧运行的水下曲线保持 1,098 点不变；两次运行在同一
  SQLite 库中共存，`pragma foreign_key_check` 为空。
- 依次删除原运行与重试运行：`backtest_runs` 及交易、回撤事件、权益、水下四张
  子表计数全部为 0，`pragma foreign_key_check` 仍为空。
- 结果：`/tmp/r05/result.json`，`ok = true`。

### AC-R03 持久化与重启

- 成功运行完成后立即返回交易 1 条、回撤事件 1 个、权益 2 点、水下 3 点与
  买卖点 2 个（`submit_success_persists_detail_and_markers`）。
- 用同一 SQLite 文件重建服务后，命名对比与运行列表仍可读；遗留的 `queued`/
  `running` 在启动时标记 `failed` 并写明中断原因（`named_comparison_survives_restart_and_tracks_missing_runs`、
  `restart_marks_queued_and_running_as_failed`）。
- 快照恢复演练中 10 个回测详情与恢复前一致；R05 运行记录同时核对
  `engine`、`worker_version` 与 `bars` 均非空。

### 限制

- 重构前的逐字段对账能发现适配器映射错误，不能证明口径本身符合外部标准；
  Sortino 已切换为 vectorbt 的下行偏差定义，与 2026-11-16 报告中的旧口径不可
  直接比较。口径依据见 `./backtest.md` 与 `./adr/0001-rust-python-vectorbt.md`。
- 只覆盖 Binance 现货、只做多、`dual_ma` 一个策略；做空、杠杆、永续不在范围内。
- vectorbt 内部实现未逐行审计，对账覆盖的是边界行为与最终数值。

## 第二层 性能基准

### AC-P01 单组 1m 全量

方法：release 构建的 Python worker，单个参数组 `fast=10/slow=30`，读取 BTCUSDT
1m 全量 4,852,639 根，包含 zip 解压、校验、指标与曲线构造；连续 5 次，取中位数。

| 运行 | 1 | 2 | 3 | 4 | 5 |
|---|---|---|---|---|---|
| 耗时（秒） | 13.234 | 13.273 | 12.253 | 12.283 | 14.505 |

- 中位数 13.234 秒，阈值 30 秒，通过。
- 单次 worker 峰值 RSS 1569.1 MB；输出 6,784 笔交易、4 个回撤事件，5 次一致。
- 2026-11-17 改为 vectorbt 原生回撤/指标后，单组一度升到 16.86 秒：
  `drawdown()` 在 485 万点 DatetimeIndex 上包装结果占 4 秒以上，改用 RangeIndex
  调同一个 accessor 后回到 13.2 秒；数值逐点相同（max abs diff 0.0）。
- 耗时波动来自同机桌面负载；5 次都低于 30 秒阈值。

### AC-P02 100 组参数扫描

方法：1m 全量，`fast=5..14 × slow=20..29` 共 100 组；同一批 worker 进程内用
fork 进程池并行，默认最多 4 个 worker。

- API 端到端：163.28 秒，从提交到 100 条运行全部进入终态，阈值 300 秒，通过；
  100/100 成功，输出约 180 MB。
- 同一批次核对：`backtest_runs.metrics_json` 与 worker 的 `result.json` 100/100
  逐字段相同；按 `params_text` 排序后的指标集 SHA-256 为 `8848c8bc0786eaf0`。
- 服务与 worker 由临时实例驱动，`max_concurrent=1`；worker 内部仍用 4 个进程。
- worker 直跑（同口径、独立批次）183.24 秒，100/100 成功，也低于 300 秒。

### AC-P03 5 并发混合查询

方法：5 个线程各自保持 HTTP keep-alive，循环请求 health、config、list、detail、
compare 五类只读接口，持续 600 秒；detail 从 10 条成功的 1m 运行中轮换，
compare 每次带 10 个运行 ID。

| 接口 | 请求数 | P50（毫秒） | P95（毫秒） | P99（毫秒） | 最大（毫秒） |
|---|---|---|---|---|---|
| health | 73,324 | 0.2 | 0.4 | 0.8 | 12.2 |
| config | 73,324 | 0.1 | 0.3 | 0.6 | 12.5 |
| list | 73,324 | 6.9 | 15.6 | 22.1 | 71.2 |
| detail | 73,321 | 17.5 | 30.0 | 37.3 | 97.8 |
| compare | 73,319 | 13.4 | 23.8 | 31.8 | 82.7 |
| 合计 | 366,612 | 6.8 | 22.9 | 31.3 | 97.8 |

- 运行 600.2 秒，610.9 请求/秒，0 错误（错误率 0%），阈值“<1% 且 P95 ≤1 秒”，
  通过。
- 服务进程峰值 RSS 175.9 MB。
- 压测只读，不包含并发提交/运行回测；服务配置 `max_concurrent=1`。

### AC-P04 资源上限与告警

方法：`scripts/resource_watch.py` 每秒采样服务进程树 RSS，按间隔累计
`/data/data`、状态库与回测输出目录的已分配磁盘；阈值 16 GiB / 500 GiB，
超限写 WARN、汇总写 ERROR，并以退出码 2 结束。负载为 release 构建的 1m 全历史
（4,852,639 根）单组回测。

| 指标 | 峰值 | 阈值 | 结果 |
|---|---|---|---|
| 进程树 RSS（服务 + worker） | 1.47 GiB | 16 GiB | 通过 |
| 已分配磁盘 | 37.35 GiB | 500 GiB | 通过 |

- 2026-11-17 复测单组 1m 全量：4,852,639 根、6,784 笔交易，81 次 RSS 采样，
  进程树峰值 1.47 GiB；磁盘项沿用 2026-11-16 的 37.35 GiB（行情归档未变）。
- 磁盘以行情归档为主（约 35 GiB），状态库 170 MB，回测输出按运行分目录。
- 告警演练：把阈值临时压到 0.01 GiB 后，脚本分别输出
  `WARN memory limit exceeded` / `WARN disk limit exceeded` 与 `ERROR SUMMARY`，
  退出码 2；默认阈值即 16 GiB / 500 GiB。

### 性能说明

- 以上都在同一台机器、同一块 NVMe 上完成，未做 CPU 绑核；操作系统页缓存是热的。
- 100 组扫描的 worker 内部并行度默认 4，可用 `TRADINGVIEW_WORKERS` 覆盖；
  并行不改变参数顺序与结果。
- 去掉 `trades.records_readable`（约 10.3 秒）和逐元素时间戳转换后，单组 1m
  才从分钟级降到 13 秒级；这是前一轮改动的直接收益。
- 2026-11-17 起回撤、水下与指标改由 vectorbt 计算。`portfolio.value()` 与
  DatetimeIndex 上的 `drawdown()` 包装在 485 万点上都是秒级开销，适配器保留
  两条线性路径：订单记录重建权益、RangeIndex 调 drawdown accessor；其余数值
  仍由 vbt 的 trades/drawdowns/returns 产出。

## 第三层 浏览器指标

### 方法

使用 headless Firefox 160 + geckodriver，通过一个只依赖标准库的 WebDriver 客户端
驱动。前端新增两个性能标记：

- `tv:first-screen`：首屏 K 线 `setData` 后等两帧记录；
- `tv:tf-switch:<timeframe>`：周期切换的 `performance.measure`。

阈值按规格执行；浏览器项取 10 次 P95，切换项逐个记录。

AC-V07 另用仓库内 `scripts/browser_layout_check.py` 驱动 Firefox 160、Firefox
159 与 Chromium 157。脚本为本机 Bearer 注入反向代理、精确校准
`innerWidth/innerHeight`、等待 `tv:first-screen`，检查横向溢出、工具栏/图表/
canvas 几何与回测抽屉边界，并保存截图。

### AC-V01 12 周期可选

- `/api/backtests/config` 返回 12 个周期，Rust 测试断言数组长度为 12
  （`config_endpoint_reports_limits`）；前端周期下拉由 `/api/timeframes`
  的真实目录扫描结果填充。
- 同区间切换演练逐个切换 12 个规格周期，均加载成功（见 AC-V03）；
  AC-D01 的 12 周期 × 1000 根官方对账同时覆盖原生数据。

### AC-C01 10 个运行对比

- 10 条 1m 长曲线（每条 2,013 点）在热缓存下连续打开 10 次，P95 94 毫秒，
  阈值 3 秒（规格目标 2 秒），通过。
- 10 条曲线均渲染，10 种颜色可区分；图例开关后曲线数 10 → 9 → 10。

### AC-C02/C05 100 组扫描与 10 组对比

- 演练 `scripts/compare_selection_check.py` 用临时服务提交 100 组参数扫描
  （fast 2–11 × slow 20–80），100 个运行全部成功且同批次；运行 ID 唯一，
  参数映射与 `params_text` 逐项精确。
- 从结果中取首尾各 5 组共 10 组进入对比：`series` 10 条、并集 1,129 点；
  请求顺序与 `series` 顺序一致；每个运行的指标与详情接口完全相等；标签含
  参数与短 ID；参数不同不产生元数据警告。
- 11 组请求返回 400，错误写明 `at most 10`，不静默截断。
- CSV 导出 10 行 × 29 列，表头与列序固定；运行 ID 保持请求顺序；14 个指标
  列与详情数值逐项一致（容差 1e-9）。
- 前端运行列表支持默认、收益、夏普与最大回撤四种顺序：纯函数
  `frontend/src/backtest-sort.ts` 由 5 项 node 测试覆盖，收益/夏普降序、
  回撤深度升序，缺失指标排最后且不修改入参数组。
- 结果：`/tmp/c05/result.json`，`ok = true`；整个演练 7.2 秒。

### AC-C03/C04/C06 对齐、警告与 CSV

- 对齐：两条曲线时间戳并集 4 点；缺失处写 `null` 并计入 `missing_points`
  （分别为 1 与 2），不做重采样或静默填充
  （`alignment_uses_union_and_marks_missing`）。
- 警告：参数不同不产生警告；标的、周期、策略版本、数据区间与成本五类差异
  各生成一条警告，共 5 条且每条都带两个运行 ID
  （`param_difference_does_not_warn`、`metadata_differences_warn_by_kind`）。
- CSV：两个运行导出表头加 2 行，共 3 行；列序固定为 29 列；参数含逗号时
  按 RFC 4180 加引号，引号与换行转义正确；10 组对比的 CSV 行数、运行顺序与
  14 个指标列已由 AC-C02/C05 演练逐项核对。

### AC-V02 首屏

每轮重启 release 服务进程（进程内 DataLoader 缓存清空）并用全新 Firefox 会话
加载，共 10 轮；热缓存为同一会话刷新 10 次。默认视窗 500 根，首屏加载 1500 根；
最新端没有未来数据，视窗落在末尾时左侧缓冲 1000 根，保存视窗位于中段时两侧各
500 根（受实际数据覆盖限制）。`tv:first-screen` 在视窗写入 `data-viewport` 后记录。

| 口径 | 10 次标记（毫秒） | P95 | 阈值 |
|---|---|---|---|
| 冷（每轮重启服务） | 235, 268, 233, 229, 239, 237, 258, 243, 221, 252 | 268 | ≤2,000 |
| 热（同一会话刷新） | 199, 222, 241, 207, 225, 191, 258, 206, 227, 223 | 258 | ≤1,000 |

两种口径都通过。20 次采样中 `data-bars` 恒为 1500，`data-visible-bars` 恒为 500，
`data-viewport` 覆盖的逻辑根数也恒为 500。冷口径只清空服务进程内的 DataLoader
缓存，操作系统页缓存仍在；首次从磁盘读取归档的极端情况没有单独测量。

### AC-V03 同区间周期切换

基线：会话写入 1h，可视区间取 1m 归档的实际首尾
`2023-06-24 04:00` 至 `2023-07-01 02:19`（UTC，165 小时），再依次切到其余
14 个周期。切换时记录当前时间区间，按时间窗分块拉取新周期数据（单页 10k 根、
总上限 100k 根），布局稳定后重新应用同一区间。

| 周期 | 耗时（毫秒） | 区间重叠率 | 可视根数 |
|---|---|---|---|
| 1m | 147 | 1.00 | 9,900 |
| 3m | 88 | 1.00 | 3,300 |
| 5m | 110 | 1.00 | 1,980 |
| 15m | 156 | 1.00 | 660 |
| 30m | 88 | 1.00 | 330 |
| 2h | 87 | 1.00 | 82 |
| 4h | 86 | 1.00 | 40 |
| 6h | 93 | 1.00 | 26 |
| 8h | 87 | 1.00 | 19 |
| 12h | 97 | 1.00 | 12 |
| 1d | 105 | 0.92 | 6 |
| 3d | 92 | 0.67 | 1 |
| 1w | 87 | 0.00 | 2 |
| 1mo | 95 | 不适用 | 1 |

- 14 次切换中位 93 毫秒、最大 156 毫秒，全部 ≤500 毫秒，通过。
- 规格的 12 个周期（1m 至 1d）中 11 个区间重叠率为 1.00；1d 为 0.92，因为
  日线对齐到 UTC 零点，窗口首尾各不足一根。12 个周期全部通过。
- 3d、1w、1mo 不在规格的 12 周期内。当窗口只有 6.9 天时，它们的单根跨度与
  窗口同量级，无法保持区间：3d 只剩 1 根、1w 落到已有 2 至 3 根、1mo 只有
  1 根。这是分辨率限制，不是视窗逻辑缺陷。
- 视窗超出该周期数据覆盖时，收敛到数据边界并保留右边缘；不会静默改回最新端。

### AC-V06 缩放、平移、十字光标、区间选择与帧率

- 1m 周期加载 10,500 根，视窗覆盖 9,939 根（2023-06-24 至 07-01），
  `minBarSpacing` 放宽到 0.01 以容纳万根级视窗。
- 连续拖拽平移采样 6 秒：60.34 fps，帧间隔 P95 17.1 毫秒，超过 33.3 毫秒的
  帧为 0，满足 ≥30 fps。
- 缩放、平移、十字光标均有截图留证，交互可用。
- 区间选择：`date-range` 工具取 2,489 根，统计栏显示
  `2489 根 · +0.85% · 高 30,998.5 · 低 29,992.5 · 量 686.61K`；与后端独立
  计算的根数、最高、最低、成交量、涨跌幅一致（最大误差 1.2e-10）。
- 刷新后区间统计与绘图恢复（服务端 1 条绘图），点击清空后统计隐藏、
  服务端绘图数为 0。

### 图表 1m 边缘加载复测（2026-11-17）

针对 1m 长历史“滚到边缘就卡”的复测，服务为 release 构建，数据为
`spot/XRPUSDT/1m`（326 万根归档），会话保存视窗
`1707548100000`–`1707563460000`，指标 PSAR `step=0.02 max=0.2`。

接口耗时（5 轮，冷口径每轮重启服务进程）：

| 请求 | 冷（毫秒） | 暖（毫秒） |
|---|---|---|
| 首屏 `limit=1500` | 9–17 | 1–3 |
| 左右边缘页 `limit=10000` | 41–62 | 8–14 |
| `/api/quality` 全历史 | 750–878 | 0.3 |

有界请求只解压覆盖窗口的归档，首屏冷读从全历史解压降到十几毫秒；质量报告
冷读仍要扫完整段归档，因此单独做了 5 分钟缓存。

浏览器（headless Chromium，1366×854，注入 `WheelEvent` 驱动缩放）：

| 操作 | 结果 |
|---|---|
| 初始加载 | 1,257 根，可视 256 根 |
| 向外滚动一格 | 1,257 → 41,257 根，新增 4 个 `/api/klines`（各 10,000 根），可视 536 根 |
| 该次边缘拼接 | 无超过 50 毫秒的长任务，帧率 58.4 fps，帧间隔 P95 16.8 毫秒 |
| PSAR 重算并重绘 41,257 根 | 41.7 毫秒 |
| 连续滚动 6 格 | 可视 21,532 根，已加载仍为 41,257 根，60 fps，无长任务 |

同一探针在修复前记录到 19.2 秒的长任务：`appendData` 对 2 万根逐根调用
`series.update()`，每次约 0.96 毫秒。现在增量超过 200 根就整段 `setData` 一次，
2 万根的写入降到 23–26 毫秒；不超过 200 根的尾部增量（回放、实时追加）仍走
逐根 update。加载本身不设硬上限，超过 5 万根时信息栏提示数据量较大。

### AC-V04 买卖点一致性

- 对一条 1h 运行的 1,090 笔交易逐一核对买卖点标记：方向、时间、价格、类型
  全部一致，标记序列无剩余、无缺失。
- 规格要求随机抽 20 笔；本次是全量核对，结论更强。

### AC-C07 命名对比保存与找回

- 新增 `backtest_comparisons` 表（`003_comparisons.sql`）与
  `GET/POST /api/backtests/comparisons`、`DELETE /api/backtests/comparisons/{id}`；
  名称 1 至 64 字符且唯一，保存 2 至 `max_compare_runs` 个已成功运行。
- 自动化演练 25 项断言全过：保存返回 201；同名、空名、65 字符、1 个 ID、重复 ID
  均 400；不存在的运行 404；重启服务后按 ID 找回且 `run_ids` 顺序一致；删除一个
  运行后 `missing_runs` 实时变为 1；删除命名对比返回 204 且列表清空。
- 浏览器演练（Firefox 160，1366×854）：勾选 2 个运行、填名保存后列表立即出现；
  重启服务并重新打开页面，点击列表项自动勾选并渲染 2 条曲线与 2 行指标表。
- 限制：名称只做 trim 与长度校验，不做大小写折叠；删除运行不会级联删除命名
  对比，读取时用 `missing_runs` 标出缺失。

### AC-V05 前端指标与接口 JSON 快照

- 输入固定为 `/api/klines` 的真实响应：U 本位永续 BTCUSDT 1h 400 根
  （`1793174400000` 至 `1794610800000`）。
- 26 个指标、42 条 series、16,800 个数值点在默认参数下与冻结快照
  `frontend/scripts/fixtures/indicator-snapshot.json` 逐点 `deepEqual`，包括
  null 暖机位；前端测试 34 项全绿。
- 数值正确性另由 `indicators.parity.test.mjs` 覆盖：SMA/EMA/RSI/ATR/MACD/
  Ichimoku 与独立参考公式逐点核对，其余指标校验 Pine 首有效 bar、值域与
  “只用当前及历史 bar”的前缀不变性。
- 快照由 `node scripts/generate-indicator-snapshot.mjs` 生成；改动指标实现时
  必须显式重建快照并复核差异。

### AC-B05 运行列表排序

方法：`scripts/backtest_sort_check.py` 启动 chromedriver，打开回测抽屉，把
`#bt-sort` 依次切到默认、收益、夏普与最大回撤，读取 `#bt-runs .bt-run` 的渲染
顺序，与接口指标按同一规则算出的期望顺序比对；规则为收益/夏普降序、回撤深度
升序、缺失指标最后，同值保持接口顺序，同标题的运行按接口顺序配对。

- 2026-11-16 容器实例（2 条成功运行：收益 31.049/4.647、夏普 4.374/0.919、
  回撤 12.587/12.659）：四种排序的实际顺序全部与期望一致，`#bt-sort` 四个选项
  齐全，默认顺序等于接口返回顺序。
- 反例：dev 实例没有运行记录时脚本以退出码 2 结束并输出 `运行数不足: 0`，
  不把空列表当成通过。
- 限制：只覆盖桌面 1366×768；运行数少于 2 或某指标全部相同时无法区分顺序，
  脚本按环境问题退出 2。

### AC-V07 浏览器布局兼容

规格要求 Chrome 与 Firefox 最新两个大版本在 1366×768 及以上无阻断性布局错误。
本轮使用 Firefox 160.0、Firefox 159.0 与 Chromium 157.0.8103.78，对 Docker
Compose 容器内的同一份前端构建逐组新建会话。

每组检查项：

- `documentElement.scrollWidth` 与 `body.scrollWidth` 不超过视口 1 px；
- 工具栏、图表容器与 canvas 均在视口内，图表宽度不低于视口 95%、高度不低于
  55%，canvas 有效尺寸大于 200×150；
- 市场、交易对、周期、布局、保存、回测与信息等关键控件可见且未横向越界；
- 打开回测抽屉后，460 px 面板的四条边均在视口内，关闭按钮可见，内容区可滚动。

| 浏览器 | 1366×768 首屏标记 | 1920×1080 首屏标记 | 横向溢出 | 图表尺寸（1366/1920） | 结果 |
|---|---|---|---|---|---|
| Firefox 160.0 | 435 毫秒 | 226 毫秒 | 0 px | 1366×687 / 1920×1035 | 通过 |
| Firefox 159.0 | 228 毫秒 | 244 毫秒 | 0 px | 1366×687 / 1920×1035 | 通过 |
| Chromium 157.0.8103.78 | 205 毫秒 | 216 毫秒 | 0 px | 1366×687 / 1920×1034 | 通过 |

- 六组全部 `failures = []`，无横向滚动条；1366 宽时工具栏自然换行，1920 宽时
  单行显示，两种布局的关键控件都可操作。
- 回测抽屉在 1366 下为 `x=906..1366`，在 1920 下为 `x=1460..1920`，均完整落在
  视口内；主视图截图在打开抽屉前采集，避免遮挡工具栏。
- 边界：只覆盖桌面 1366×768 及以上，不含移动端与触摸手势；headless 字体栅格
  可能与有头模式有细微差异。截图留在基准机 `/tmp/v07/`，未提交到仓库。

## 第四层 工具链、安全与运维

### 依赖与代码检查

- `cargo audit --no-fetch`：退出码 0，扫描 209 个 crate，加载 1,509 条 advisory，
  未发现漏洞。advisory 库提交为 `02e2517`；在线更新通过 socks5 代理完成，
  `cargo audit` 自身不支持 socks 代理。
- `cargo deny`：仓库没有 `deny.toml`，本轮不适用。
- `unsafe`：`src` 与 `tests` 中 0 处，Miri 不适用于当前代码。
- `cargo fmt --check`、`clippy -D warnings`、`cargo test`、`cargo doc` 在最终提交前
  统一执行：`cargo test -p tradingview-backend` 共 137 项通过（lib 108、
  `backtest_test` 8、`http_compression_test` 3、`integration_test` 7、
  `udf_contract_test` 10、`udf_test` 1），0 失败、0 忽略；`cargo doc` 以
  `RUSTDOCFLAGS="-D warnings"` 运行 `--no-deps --document-private-items`，无警告。

### AC-S01 网络边界与认证

- 默认配置 `host = "127.0.0.1"`，只监听回环；从同一台机器的 Tailscale 地址
  `100.100.24.58:3011` 连接被拒绝。
- 新增 `auth_token` 配置。未配置时行为不变（仅回环可用）；配置后启动日志提示
  认证已启用。把 `host` 改成非回环地址且未配令牌时，进程在启动阶段直接失败：
  实测 `host = "0.0.0.0"`、`auth_token = ""` 退出码 1，报
  `auth_token must not be empty`。
- 启用后 `/api/*` 与 `/udf/*` 都要求 `Authorization: Bearer <token>`，或浏览器
  Basic 登录（用户名任意，密码为令牌）。`/api/health` 与 `OPTIONS` 预检豁免；
  静态页面不拦，否则登录框无法弹出。
- 实测（127.0.0.1:3012，令牌 `verify-token`）：`/api/health` 200；`/api/layouts`
  无凭据 401、Bearer 200、Basic 200、错误令牌 401；`/udf/history` 无凭据 401。
- 401 响应带 `WWW-Authenticate: Basic realm="tradingview"`，浏览器可直接弹框。
- 认证是单令牌、无会话、无多用户；不内置 TLS，公网部署仍需反向代理终止 TLS。
  Basic 在明文 HTTP 下会暴露令牌，非回环场景应优先用 Bearer 或 HTTPS。

### AC-S02 非法输入与可配置上限

- 23 个非法用例全部返回 4xx，无 500：
  - 400：非法 JSON、非法周期、fast ≥ slow、负手续费、零初始资金、空区间、
    做空方向、非现货、参数网格超限、非法 UTF-8、compare 只有 1 个/11 个/重复 ID；
  - 413：超过请求体上限；
  - 404：不存在的运行、非法 UUID、日志、重试、删除、compare 中不存在的运行、
    不存在的交易对或周期；
  - 422：缺字段的 JSON（axum 反序列化失败），仍是 4xx；
  - 合法请求仍为 200，运行最终 `succeeded`。
- `max_body_bytes` 成为配置项，合法区间 1 KiB 至 64 MiB，默认 2 MiB，直接传给
  `DefaultBodyLimit`。
- 实测（上限 1 KiB）：512 B 的偏好请求 200，2 KiB 返回 413
  `length limit exceeded`；默认 2 MiB 配置下 3 MiB 请求仍为 413。

### AC-P06 失败日志与重试

- 假 worker 以退出码 3 失败后，运行状态为 `failed`，错误字段包含
  `boom from fake worker`；`GET /api/backtests/{id}/log` 返回同一段 stderr。
- 重试返回 202，生成新的运行 ID 与新批次；原运行保留为 `failed`，历史详情
  仍可读；重试后的运行继续交给同一个 worker 执行
  （`failure_exposes_log_and_retry_creates_new_run`）。

### AC-P07 数据库快照恢复

方法：`scripts/snapshot_restore_check.py` 用 SQLite 在线 backup API 生成快照，
以副本在 3013 端口启动临时服务，抽样 100 条 1m 行情与最近 10 个成功运行详情，
写入标记布局后停服，用快照覆盖数据库并重启复核。

- 快照 170,516,480 字节，耗时 0.08 秒；来源库、快照与恢复库的
  `pragma integrity_check` 均为 `ok`。
- 恢复前后 9 张表行数完全一致：`backtest_runs` 117、`backtest_trades` 672,383、
  `backtest_equity`/`backtest_underwater` 各 226,221、`backtest_drawdown_events`
  1,067、`backtest_comparisons` 0、`drawings` 1、`preferences` 1、`layouts` 0。
- 100 条 K 线 JSON 的规范化哈希一致；10 个运行的 ID 顺序与详情哈希一致。
- 快照之后写入的标记布局在恢复后消失，证明恢复覆盖了旧库；两次启动都通过健康检查。

### AC-P05 Docker Compose 部署

仓库新增根 `Dockerfile`、`docker-compose.yml`、`docker/entrypoint.sh`、
`.dockerignore` 与 `.env.example`。镜像分三段构建：

- 前端：Node 22，`npm ci` 后执行 `npm run build`；
- 后端：Rust 1.100 slim，`cargo build --release --locked --bin tradingview`；
- 运行时：Python 3.12 slim，安装本地 `python/` 包（vectorbt 1.2.0、numpy
  2.5.5、pandas 3.1.1），复制二进制与前端产物。

Cargo 依赖 `../data-reader/crates/binance-core` 与 `dukascopy-core`。Compose 用
`additional_contexts` 把兄弟仓库作为只读命名上下文，Dockerfile 只复制
`Cargo.toml`、`Cargo.lock` 与 `crates/`，不会发送 data-reader 的 1.6 GB target，
也不会把 35 GiB 行情带进构建上下文。

运行时安全与状态：

- 容器用户 uid/gid 10001；根文件系统只读；`/tmp` 为 256 MiB tmpfs；
- 行情 `/data/data` 只读绑定到 `/data`；
- SQLite、回测请求/结果/日志放在命名卷 `tradingview-rs-state` 的 `/state`；
- 入口脚本从环境变量生成 `/tmp/tradingview.toml`，固定 `host = "0.0.0.0"`，
  `TV_AUTH_TOKEN` 缺失时退出码 64；服务端再校验非空与空白字符；脚本先
  `umask 077`，配置文件实测权限 600，同容器其他进程读不到令牌；
- 健康检查每 30 秒请求 `/api/health`，`no-new-privileges` 开启。

演练步骤与结果：

- 清空容器与卷后执行一条
  `docker compose up -d --build --wait --wait-timeout 240`，6.3 秒后容器 healthy，
  端口映射 `3101→3001`（避开基准机已有的 3001 debug 实例）。
- 无令牌 `GET /api/backtests` 返回 401，Bearer 令牌返回 200；
  `/api/health` 免认证返回 200。
- 提交 2 个真实 BTCUSDT 1h 回测（2024-01-01 至 2024-03-01，fast/slow 为
  10/30 与 20/60），均 `succeeded`：各 1,440 根，33 笔与 14 笔交易，总收益
  4.6474% 与 31.0488%。
- 9 张表行数：`backtest_runs` 2、`backtest_trades` 47、`backtest_equity`/
  `backtest_underwater` 各 2,880、`backtest_drawdown_events` 38，其余 0。
- `docker compose restart` 后，两个运行详情规范化哈希与 9 张表行数完全一致；
  再 `down` + `up` 重建容器与网络后仍一致。
- 容器内复核：uid 10001，vectorbt 1.2.0 / numpy 2.5.5 / pandas 3.1.1；配置为
  `host=0.0.0.0`、`db_path=/state/tradingview.db`、worker `/app/python`。
- 2026-11-16 前端排序版更新镜像：`docker compose build` 只重跑运行时 pip 层
  （`TV_PIP_INDEX_URL` 指向阿里云源，10.5 分钟），后端与前端阶段命中缓存；
  `docker compose up -d --wait` 后 5 秒 healthy。`/api/health` 200，无令牌
  `/api/backtests` 401，Bearer 令牌 200；`/` 的 sha256 与 `frontend/dist/index.html`
  一致（`899249…f80f`），`assets/index-Ctm9ChmA.js` 与 `index-Drb9h2HW.css`
  逐字节一致，旧 `index-DBl5X4KH.js` 已被替换；状态卷中的 `tradingview.db`、
  `backtests/` 与既有运行记录保留。

边界：演练在同一台基准机上完成，不是独立物理机；基础镜像已预先拉取；构建期
需要访问 npm、crates.io 与 PyPI。代理只监听回环时需把 `TV_BUILD_NETWORK` 设为
`host`；官方 PyPI 慢时用 `TV_PIP_INDEX_URL` 指向镜像站。这些限制不影响
“空状态卷 + 单条 Compose 命令启动并持久化”的结论。

## 未覆盖与偏差

数据层 AC-D01 至 AC-D07 均已在本报告第零层覆盖；其余偏差与风险如下。

其他风险：

- Python 3.14 在“多线程进程里 fork”时会发 `DeprecationWarning`；参数扫描的
  fork 进程池当前可用，但未来 Python 移除 fork 或禁用该组合时需要改 `spawn`，
  届时行情表需要改为共享内存或重新读取。
- 浏览器与性能结果来自单机、热页缓存；没有做冷盘、低内存、限核等降级场景。
- Docker 演练限于单机 x86_64；未做跨主机、跨架构与离线镜像导入。V07 只覆盖
  桌面 1366×768 及以上，不含移动端与触摸手势。
- AC-V03 的“同区间”依赖数据覆盖：若目标周期在该时间窗内没有归档，只能收敛到
  已有数据边界。本轮 12 个规格周期在 2023-06-24 至 07-01 都有数据；3d/1w/1mo
  的窗口小于一根 bar，保持区间没有意义。

## 复现

正确性对账与门禁可直接在仓库内复跑：

```bash
cargo fmt -- --check
cargo clippy -p tradingview-backend --all-targets -- -D warnings
cargo test -p tradingview-backend
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items

cd python
.venv/bin/python -m unittest discover -s tests -v

cd ../frontend
npm run build
npm test

cd ..
# AC-R05：keep_equity=false、重试与级联删除
python3 scripts/retention_check.py \
    --binary target/release/tradingview --data-dir /data/data \
    --work-dir /tmp/r05 --port 3015

# AC-C02/C05：100 组扫描、10 组对比、11 组拒绝与 CSV
python3 scripts/compare_selection_check.py \
    --binary target/release/tradingview --data-dir /data/data \
    --work-dir /tmp/c05 --port 3016

# AC-B05：运行列表排序（需已启动的实例与 chromedriver）
python3 scripts/backtest_sort_check.py \
    --binary /usr/bin/chromium --driver /usr/bin/chromedriver \
    --url http://127.0.0.1:3101 --auth-token "$TV_AUTH_TOKEN" --label container-sort
```

数据层验收：

```bash
# AC-D01：12 周期 × 1000 根对 Binance 官方 K 线（需要出口代理）
python/.venv/bin/python scripts/data_official_check.py \
    --start 2024-01-01 --proxy http://127.0.0.1:1081 --out /tmp/d01.json

# AC-D02：12 周期 × 两个月档逐字段对账
python3 scripts/source_field_check.py \
    --reader /data/data-reader/target/release/binance-core --out /tmp/d02.json

# AC-D03：空目录连续导入两次，核对幂等性
python3 scripts/data_reimport_check.py \
    --downloader /data/data-downloader/target/release/binance-data-downloader \
    --reader /data/data-reader/target/release/binance-core \
    --work-dir /tmp/d03 --clean --out /tmp/d03.json

# AC-D04：注入异常并核对计数、样例与失败信息
python3 scripts/data_anomaly_check.py \
    --reader /data/data-reader/target/release/binance-core \
    --work-dir /tmp/d04 --out /tmp/d04.json

# AC-D05：1m 聚合对原生 1h/1d
python/.venv/bin/python scripts/data_cross_timeframe_check.py \
    --data-dir /data/data --pair BTCUSDT --start 2024-01-01 \
    --end 2025-01-01 --hours 100 --days 10 --out /tmp/d05.json

# AC-D06：前 100 标的 × 12 周期全量导入；空目录才是冷启动
python3 scripts/data_import_bench.py \
    --downloader /data/data-downloader/target/release/binance-data-downloader \
    --output-dir /data/d06-bench --symbols 100 --concurrency 64 \
    --proxy http://127.0.0.1:1081 --out /tmp/d06.json

# AC-D07：一年 1m 查询 P95，预热后 100 次
python/.venv/bin/python scripts/data_query_bench.py \
    --data-dir /data/data --pair BTCUSDT --start 2024-01-01 \
    --end 2025-01-01 --runs 100 --out /tmp/d07.json
```

Docker 部署与资源告警、快照恢复可直接复跑：

```bash
# AC-P05：需要并排的 ../data-reader；先写入 TV_AUTH_TOKEN
cp .env.example .env
docker compose up -d --build --wait --wait-timeout 240
docker compose ps

# 后续 curl / 浏览器脚本从 .env 读取令牌
set -a; . ./.env; set +a

# AC-P04：采样服务进程树与磁盘，超限退出码 2
python3 scripts/resource_watch.py --pid <服务 PID> \
    --path /data/data --path <状态库> --path <回测输出目录>

# AC-P07：在线快照 → 临时实例 → 覆盖恢复 → 复核
python3 scripts/snapshot_restore_check.py \
    --source-db <状态库> --work-dir /tmp/p07 --data-dir /data/data

# AC-V07：单组布局检查；其余组合换浏览器、二进制、驱动与视口
python3 scripts/browser_layout_check.py \
    --browser firefox --binary /usr/bin/firefox \
    --driver <geckodriver> --url http://127.0.0.1:3001 \
    --auth-token "$TV_AUTH_TOKEN" --width 1366 --height 768 \
    --label firefox-160-1366x768 --screenshot /tmp/v07/firefox-160-1366x768.png
```

性能与浏览器计时口径见上文，API 与 Compose 变量说明见 `./backtest.md`。
复跑 V07 需要 Firefox 160/159、Chromium 157 与匹配的 geckodriver/ChromeDriver。
复跑性能项时需确认：

- 用 release 构建，Python 解释器装有 vectorbt 1.2.0；
- 单组 1m 请求覆盖 2017-08-17 起的全部归档；
- 100 组扫描为 `fast=5..14 × slow=20..29`；
- 并发压测只打只读接口，持续 600 秒，5 个 keep-alive 连接；
- 浏览器读取页面内 `tv:first-screen` 与 `tv:tf-switch:*` 条目。
