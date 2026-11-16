# 回测子系统验证报告

2026-11-16 在基准机上对回测子系统做了四层验证：正确性对账、性能基准、
浏览器指标、工具链与运维安全。本文只记录实际运行过的用例；未运行项与规格偏差
在文末单列，不能把本文当作全部验收项的签字确认。

## 结论

| 层 | 验收项 | 结果 | 关键数据 |
|---|---|---|---|
| 第一层 正确性 | AC-B01/B02/B03/B06/B07、AC-R01/R02/R04 | 通过 | 35 项 unittest 全绿；逐笔、权益、回撤事件与指标和独立实现一致 |
| 第二层 性能 | AC-P01/P02/P03/P04 | 通过 | 1m 全量单组中位 13.38 秒；100 组 164.74 秒；5 并发 P95 22.9 毫秒、0 错误；压力回测峰值 RSS 1.587 GiB、磁盘 37.35 GiB |
| 第三层 浏览器 | AC-C01/V02/V03/V04/V06/C07/V05/V07 | 通过 | 对比 P95 94 毫秒；首屏冷 268 毫秒、热 258 毫秒；同区间切换中位 93 毫秒；9,939 根 60.34 fps；命名对比跨重启找回；26 个指标 16,800 点与接口快照一致；三浏览器六组合布局检查全过 |
| 第四层 工具链、安全与运维 | AC-S01/S02/P04/P05/P07 | 通过 | 依赖审计 0 漏洞；Bearer/Basic 与可配置上限生效；23 个非法用例 4xx；Compose 单命令启动 6.3 秒，重启/重建后 2 个运行与 9 张表一致；快照恢复后 9 张表行数一致、10 个回测详情一致 |

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
- 计时：性能项取 10 次或 5 次的 P95/中位数；浏览器耗时取自页面内的
  `performance.mark/measure`，在 `setData`、应用首屏视窗并等两帧后记录。
- 缓存：性能与浏览器测试都在同一台机器上连续运行，操作系统的页缓存是热的；
  “冷”只表示服务进程的 DataLoader 缓存为空，见各节说明。

## 第一层 正确性对账

### 方法

`python/tests/reference.py` 是一个不依赖 vectorbt 的独立参考实现，只用 pandas 与
numpy 重算双均线只做多策略。对账口径在实现前先冻结：

- 信号在 T 收盘比较 fast/slow 均线，目标仓位右移一根 bar，在 T+1 开盘成交；
- 入场数量为 `现金 / (1 + 手续费率) / 买价`，买价为 `open × (1 + 滑点率)`；
- 卖出价为 `open × (1 - 滑点率)`，卖出后现金为成交额减手续费；
- 每根 bar 的权益按收盘价 mark-to-market，未平仓头寸按最后一根收盘价估值；
- 回撤按收盘权益计算，峰值到谷底再到恢复为一个事件。

### 用例与结果

| 用例 | 内容 | 结果 |
|---|---|---|
| 合成随机游走 | 0 成本与 10 bps 手续费 + 5 bps 滑点两组 | 逐笔成交、权益、回撤事件、指标与参考实现一致 |
| 固定净值序列 | 10 组：单调上涨、单次深回撤、多次回撤、未恢复、零回撤等 | 事件识别与深度一致 |
| 成本手算 | 单笔全仓进出，按上述公式独立核算 | 价格/数量误差 ≤1e-10，盈亏误差 ≤1e-8 |
| 无前视 | 只在最后一根 bar 出信号，检查首笔成交晚于 slow 窗口 | 通过 |
| 降采样 | 抽样点必须来自原始曲线，水下极值保留 | 抽样误差 0 |
| 确定性 | 同输入连续运行 3 次，对结果 JSON 做规范化哈希 | 3 次哈希完全相同 |
| 真实数据 | BTCUSDT 1h 2024 年上半年 4,368 根，引擎对参考实现 | 逐笔与指标一致 |
| 跨周期 | 1m 聚合出 1h/1d，与原生归档逐根对比 | OHLC 一致 |

执行结果：`Ran 35 tests in 7.068s`，退出码 0。真实数据用例由环境变量
`TRADINGVIEW_REAL_DATA` 门控，未设置时跳过。

### 发现并修复的缺陷

1. 权益按开盘价估值。旧实现用 `from_orders(targetpercent)` 只接收一个价格序列，
   成交价与估值价都是开盘价；实际权益应在收盘 mark。修复为
   `from_signals(close=收盘价, price=开盘价)`，再按订单记录逐 bar 重建收盘权益。
   回归用例为 `test_equity_is_marked_to_close_not_open`。
2. 交易时间戳逐个调用 `Timestamp.timestamp()`，在百万级索引上开销明显；改为
   索引直接转毫秒整数。该改动只影响性能，不改变数值。
3. 逐行解析 1m 归档约 485 万行，是单组回测的主要耗时；改为 pandas C 解析器
   整块读取后再向量化校验、过滤、拼接。

### 限制

- 参考实现与引擎共享同一组口径决定，它能发现实现错误，不能证明口径本身符合
  外部标准；口径依据见 `./backtest.md` 与 `./adr/0001-rust-python-vectorbt.md`。
- 只覆盖 Binance 现货、只做多、`dual_ma` 一个策略；做空、杠杆、永续不在范围内。
- vectorbt 内部实现未逐行审计，对账覆盖的是边界行为与最终数值。

## 第二层 性能基准

### AC-P01 单组 1m 全量

方法：release 构建的 Python worker，单个参数组 `fast=10/slow=30`，读取 BTCUSDT
1m 全量 4,852,639 根，包含 zip 解压、校验、指标与曲线构造；连续 5 次，取中位数。

| 运行 | 1 | 2 | 3 | 4 | 5 |
|---|---|---|---|---|---|
| 耗时（秒） | 13.658 | 12.622 | 13.048 | 13.383 | 13.469 |

- 中位数 13.383 秒，阈值 30 秒，通过。
- 进程峰值 RSS 1724.5 至 1726.6 MB。
- 每次输出 6,784 笔交易、4 个回撤事件，5 次完全一致。
- 第 1 次 13.658 秒，第 2 次 12.622 秒，之后在 13.0 至 13.5 秒间波动；差异来自
  系统负载，首次冷盘读取没有单独测量。

### AC-P02 100 组参数扫描

方法：1m 全量，`fast=5..14 × slow=20..29` 共 100 组；同一批 worker 进程内用
fork 进程池并行，默认最多 4 个 worker。

- worker 直跑：160.2 秒，100/100 成功，峰值 worker RSS 1595.3 MB；结果指纹与
  串行版本逐字段一致。
- API 端到端：164.74 秒，从提交到 100 条运行全部进入终态，阈值 300 秒，通过；
  100/100 成功，逐组指标与直跑结果逐字段一致。
- 服务进程峰值 RSS 112.2 MB。

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
| 进程树 RSS（服务 + worker） | 1.587 GiB | 16 GiB | 通过 |
| 已分配磁盘 | 37.35 GiB | 500 GiB | 通过 |

- 运行 `ecc857c4-5dce-4f70-9f81-c65f74e04585` 成功，4,852,639 根、6,784 笔交易，
  与 AC-P01 的单组结果一致；16 次 RSS 采样覆盖完整回测。
- 磁盘以行情归档为主（约 35 GiB），状态库 170 MB，回测输出按运行分目录。
- 告警演练：把阈值临时压到 0.01 GiB 后，脚本分别输出
  `WARN memory limit exceeded` / `WARN disk limit exceeded` 与 `ERROR SUMMARY`，
  退出码 2；默认阈值即 16 GiB / 500 GiB。

### 性能说明

- 以上都在同一台机器、同一块 NVMe 上完成，未做 CPU 绑核；操作系统页缓存是热的。
- 100 组扫描的 worker 内部并行度默认 4，可用 `TRADINGVIEW_WORKERS` 覆盖；
  并行不改变参数顺序与结果。
- 去掉 `trades.records_readable`（约 10.3 秒）和逐元素时间戳转换后，单组 1m
  才降到当前水平；这是本次改动的直接收益。

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

### AC-C01 10 个运行对比

- 10 条 1m 长曲线（每条 2,013 点）在热缓存下连续打开 10 次，P95 94 毫秒，
  阈值 3 秒（规格目标 2 秒），通过。
- 10 条曲线均渲染，10 种颜色可区分；图例开关后曲线数 10 → 9 → 10。

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
  null 暖机位；前端测试 29 项全绿。
- 数值正确性另由 `indicators.parity.test.mjs` 覆盖：SMA/EMA/RSI/ATR/MACD/
  Ichimoku 与独立参考公式逐点核对，其余指标校验 Pine 首有效 bar、值域与
  “只用当前及历史 bar”的前缀不变性。
- 快照由 `node scripts/generate-indicator-snapshot.mjs` 生成；改动指标实现时
  必须显式重建快照并复核差异。

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
  统一执行，结果见提交记录。

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

边界：演练在同一台基准机上完成，不是独立物理机；基础镜像已预先拉取；构建期
需要访问 npm、crates.io 与 PyPI。代理只监听回环时需把 `TV_BUILD_NETWORK` 设为
`host`。这些限制不影响“空状态卷 + 单条 Compose 命令启动并持久化”的结论。

## 未覆盖与偏差

| 项 | 状态 | 说明 |
|---|---|---|
| AC-D01/D02/D06 | 不在本轮 | 数据导入正确性与全量导入性能 |

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
TRADINGVIEW_REAL_DATA=/data/data /data/vbt/.venv/bin/python -m unittest discover -s tests -v

cd ../frontend
npm run build
npm test
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
