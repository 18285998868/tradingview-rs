# Changelog

## Unreleased

### Changed

- `python/backtest_worker/data.py`：`load_klines` 在读取前按 `[start, end)` 裁剪月/日归档，一年 1m 查询从 130 个文件降到 12 个，P95 2,995 ms → 362 ms；新增 3 项单测覆盖月档、日档与无交集区间。
- `src/app/udf.rs` 拆为 `src/app/udf/`（`symbol` / `resolution` / `history` / `price` / `protocol` / `handlers`），`/udf/*` 响应形状除下列协议修复外不变。
- `/udf/history`：`to` 改为右开区间；`countback` 优先于 `from`；空窗口在可回查时返回 `no_data.nextTime`。
- `/udf/symbols`：`pricescale` 由最粗周期的样本价格小数位推导（静态值作下限）；`has_intraday` 只统计日内周期；新增 `1W`/`1M`/`3D` 与 `weekly_multipliers`/`monthly_multipliers`；dukascopy `session` 改为 `0000-0000`。
- `/udf/config`：`supported_resolutions` 增加 `3D`、`1W`、`1M`。
- 修正 `src/config.rs` 指向私有函数的 rustdoc 链接，`RUSTDOCFLAGS="-D warnings" cargo doc` 重新可用。
- `DataLoader` 缓存改为按字节计权：权重取六列容量之和（约 48 字节/bar），上限 2 GiB，替换原来的 200 条上限；过期策略改为 `time_to_idle` 10 分钟 + `time_to_live` 30 分钟，替换原来的 5 分钟 TTL。
- 同一 key 的并发未命中改为 single-flight：`try_get_with` 只让一个线程读盘，其余请求等它的结果；错误经可克隆的 `LoadError` 分发，`NotFound`/`InvalidParam` 的 404/400 分类保持不变。
- `list_pairs` / `list_timeframes` 的目录扫描结果加 60 秒 TTL 缓存（交易对按市场、周期按市场 + 交易对），`/api/pairs`、`/api/timeframes`、`/udf/search`、`/udf/symbols` 不再每次都递归 `read_dir`；扫描失败不入缓存，`invalidate_cache` 同时清空 K 线与目录缓存。
- 开启 gzip 响应压缩（`tower-http` 的 `compression-gzip`，`Fastest` 级别）：`/api/klines` 1m 全量 4.3 MB → 1.3 MB，release 下多花约 5–10 ms CPU；<32 字节与图片/SSE 响应不压。
- 新增依赖：`async-compression`（`tower-http` 的压缩实现），`Cargo.lock` 随之更新；`flate2` 只作为 dev-dependency 用于测试解压。
- 前端图表增量刷新：`updateData` / `appendData` 在“前缀不变、只追加或改写最后一根”时改用 `series.update()`，不再整段 `setData`；指标序列同样只更新尾部，回放逐根前进不再重建整条线。
- `prependData` 仍整段 `setData`（lightweight-charts 没有 prepend API），但只转换新增的 K 线前缀。
- 指标条 DOM 只在增删/换序时重建；回放每帧不再重写 `innerHTML`。
- 首屏预取改为默认视窗 500 根、首次加载 1500 根；保存视窗位于中段时两侧各预取 500 根，最新端左侧缓冲 1000 根。`tv:first-screen` 在视窗应用并写入 `data-viewport` 后才记录。
- 周期切换保持当前时间区间：切换前记录视窗，按时间窗分块拉取（单页 10k、总上限 100k），布局稳定后重新应用同一区间；目标周期数据不足时收敛到数据边界。`tv:tf-switch:<tf>` 同样在应用视窗后记录。
- 1m 万根视窗支持：`minBarSpacing` 放宽到 0.01，首屏/无保存视窗时显示最新 500 根，滚动到距数据边缘不足 500 根时按页补载。
- `date-range` 绘图选择实时统计根数、涨跌幅、最高、最低与成交量；刷新后按服务端绘图恢复，清空绘图后统计隐藏。
- 新增 `auth_token` 配置：启用后 `/api/*` 与 `/udf/*` 要求 `Authorization: Bearer <token>` 或 Basic（用户名任意，密码为令牌）；`/api/health` 与 `OPTIONS` 豁免，静态页面不拦；非回环 `host` 未配置令牌时启动失败。
- 请求体上限改为 `max_body_bytes` 配置项（1 KiB 至 64 MiB，默认 2 MiB），传给 `DefaultBodyLimit`；原先固定 2 MiB。

- `Cargo.toml`：tokio 增加 `process` / `time`，用于 Python worker 子进程、超时与增量读取 stdout/stderr；未新增其他 crate。
- `src/config.rs`：新增 `[backtest]` 配置段（`enabled`/`python_bin`/`worker_dir`/`output_dir`/`timeout_secs`/`max_concurrent`/`max_compare_runs`/`max_grid_runs`/`max_curve_points`/`max_trades`/`keep_equity`），缺省启用；`enabled = false` 时 `/api/backtests*` 返回 503。
- `src/app/state.rs`：启动时同时执行 `001_init.sql` 与 `002_backtest.sql`；删除回测运行时级联清理交易、回撤事件与曲线。
- 前端：原策略面板位置改为回测面板（`frontend/src/backtest.ts`），主图支持买卖点叠加与单独清除；会话与布局 payload 移除 `strategy` 字段。
- `README.md`、`STEP.md`、`CHANGELOG.md`、`docs/`：同步回测边界、接口、存储、限制与验证方式。
- `python/backtest_worker/data.py`：归档读取从逐行 `csv` 解析改为 pandas C 解析器整块读取，再向量化做有限值/正价格/负成交量/OHLC 包络校验、时区过滤、日归档覆盖与拼接；1m 全量约 485 万行的读取降到秒级。
- `python/backtest_worker/engine.py`：成交与估值分离，改用 `from_signals(close=收盘价, price=开盘价)`，并按订单记录线性重建逐 bar 收盘权益；移除 `trades.records_readable`（约 10.3 秒）与逐元素 `Timestamp.timestamp()`。
- `python/backtest_worker/cli.py`：参数扫描在多组时用 fork 进程池并行，默认最多 4 个，`TRADINGVIEW_WORKERS` 可覆盖；保持参数顺序，单组失败仍只影响该组。
- `STEP.d/06-frontend-incremental.md`：验证命令改用仓库内 `frontend/node_modules`，不再引用临时目录。

### Added

- 数据层验收：`docs/data.md` 记录归档布局、12 字段映射、UTC 毫秒与 2025 年起微秒归一、质量标记口径；新增七个可复跑脚本，覆盖 AC-D01 官方 K 线对账、AC-D02 源文件逐字段对账、AC-D03 重复导入幂等性、AC-D04 异常注入、AC-D05 跨周期一致性、AC-D06 前 100 标的 12 周期月档导入计时与 AC-D07 一年 1m 查询 P95。
- Docker Compose 部署：根 `Dockerfile` 分三段构建 Node 前端、Rust release 二进制与 Python/vectorbt 运行时；`docker-compose.yml` 通过 `additional_contexts` 引入兄弟 `data-reader` 的两个 core crate，命名卷保存 SQLite 与回测产物，入口脚本从 `TV_AUTH_TOKEN` 生成配置；容器非 root、根文件系统只读、行情只读挂载并带健康检查。
- `scripts/browser_layout_check.py`：AC-V07 浏览器布局检查，支持 Firefox/Chromium WebDriver、Bearer 注入代理、视口校准、横向溢出/工具栏/图表/回测抽屉断言与截图。
- `backtest_comparisons` 表（`migrations/003_comparisons.sql`）与 `/api/backtests/comparisons`：命名对比支持列表、保存与删除；名称 1–64 字符且唯一，只接受 2 至 `max_compare_runs` 个已成功运行，运行缺失时列表返回实时 `missing_runs`。
- 前端对比视图的命名保存/找回/删除：`frontend/src/backtest.ts` 增加名称输入、已保存列表与一键重新对比，重启后可从列表找回。
- AC-V05 指标快照回归：`frontend/scripts/fixtures/klines-400.json` 为 `/api/klines` 真实响应，`indicator-snapshot.json` 覆盖 26 个指标 16,800 个数值点；`indicator-snapshot.test.mjs` 逐点比对，`generate-indicator-snapshot.mjs` 负责重建。
- `scripts/resource_watch.py`：AC-P04 进程树 RSS 与磁盘峰值监控，阈值默认 16 GiB / 500 GiB，超限写 WARN/ERROR 并以退出码 2 结束。
- `scripts/snapshot_restore_check.py`：AC-P07 快照恢复演练，在线备份、临时实例、覆盖恢复与表行数/行情/回测详情比对一键完成。
- `GET /api/quality?market_type=&pair=&timeframe=`：基于现有 `DataLoader` 缓存列集输出数据质量报告，含 bar 数/首尾时间、按周期步长推算的应有 bar 数与覆盖率、缺口段数/最长缺口/最多 20 条缺口明细、零/负成交量、OHLC 包络异常、非正价格，以及清洗阶段丢弃的重复时间戳/非有限值行数；`1mo` 按自然月计算缺口，报告不写状态库。
- 前端信息栏数据质量徽标：`frontend/src/quality.ts` 把报告压成覆盖率 + 缺口/零量/异常文本，覆盖率与异常决定绿/黄/红三档，零成交单独出现不降级；`loadQuality` 在整段 K 线加载后异步拉取，`invalidateQualityBadge` 在切换交易对/市场/周期时递增序号并立即隐藏旧徽标，过期响应直接丢弃。
- `KlineColumns::clean_stats`：记录 `from_parts` 丢弃的非有限值行与重复时间戳行；切片视图不携带该统计。
- `tests/udf_contract_test.rs`：锁定 `/udf/config`、`/udf/symbols`、`/udf/search`、`/udf/history` 的 JSON 契约与官方语义（`to` 右开、`countback` 优先、`nextTime`、`pricescale` 采样、周线能力）。
- `STEP.md` 与 `STEP.d/`：拆分范围、协议修复清单与官方语义依据。
- 缓存测试：权重按字节换算、权重确实注册进缓存、8 线程并发未命中只读一次盘、错误分类往返。
- 目录缓存测试：扫描结果在 TTL 内复用、`invalidate_cache` 后立刻可见、按市场分键。
- `STEP.d/03-kline-cache.md`：缓存硬化的设计取舍与验证命令。
- `STEP.d/04-catalog-cache.md`：目录扫描缓存的设计与验证。
- `STEP.d/05-compression.md`：gzip 级别取舍与 ETag 决策。
- `tests/http_compression_test.rs`：gzip 往返字节相等、小响应不压、`identity` 不压。
- `src/incremental.ts` + `scripts/chart-incremental.test.mjs`：图表尾部增量判定的纯函数与测试（追加、最后一根改写、缩短/乱序回退、指标前缀兼容）。
- `src/app/auth.rs`：Bearer/Basic 认证中间件、Base64 解码与常量时间比较，含缺失/错误令牌与 `WWW-Authenticate` 测试。
- `frontend/src/viewport.ts` + `scripts/viewport.test.mjs`：分块合并去重、视窗取数窗口、区间统计与 100k 根上限的纯函数与测试。
- `STEP.d/06-frontend-incremental.md`：前端增量更新范围与指标重算成本。
- `python/backtest_worker/`：Binance 现货 zip 读取（12 个原生周期，2025 起的微秒时间戳归一为毫秒）、双均线只做多 vectorbt 1.2.0 引擎（T 收盘信号、T+1 开盘成交）、回撤事件与指标、保端点降采样、参数扫描与 CLI；单组失败只标记该组。
- `python/tests/`：35 个 unittest，覆盖数据读取（含 2025 起微秒时间戳）、引擎无前视、收盘权益、成本公式、回撤事件、指标公式、降采样、确定性与 CLI 契约。
- `python/tests/reference.py`：不依赖 vectorbt 的 pandas/numpy 参考实现，对合成随机游走、固定净值序列与真实 BTCUSDT 1h 数据逐笔对账。
- `docs/validation.md`：五层验证报告，记录数据对账、正确性对账、性能基准、浏览器指标、工具链与安全的方法、数据、偏差和未覆盖项。
- 前端 `performance.mark/measure`：`tv:first-screen` 与 `tv:tf-switch:<timeframe>`，用于首屏和周期切换计时。
- `src/app/backtest/`：模型与参数网格、任务状态机与并发许可、Python 子进程（超时/日志/失败重试）、SQLite 事务落库、对比警告与时间并集、CSV、HTTP 路由。
- `migrations/002_backtest.sql`：`backtest_runs`/`backtest_trades`/`backtest_drawdown_events`/`backtest_equity`/`backtest_underwater` 五张表与索引。
- `/api/backtests*`：config/提交/列表/详情/日志/重试/删除/对比/CSV；参数扫描共享 `batch_id`，上限 100 组；对比 2–10 个运行，少于 2 或多于 10 返回 400。
- `frontend/src/backtest.ts`：参数表单、参数扫描、运行轮询、详情、权益/水下 SVG、回撤事件、交易表、10 个对比与图例开关、CSV/日志/重试/删除。
- `tests/backtest_test.rs`：7 个集成测试，用假 worker 驱动状态机、参数网格、对比上限、失败恢复与重试。
- `SPEC.md`、`docs/adr/0001-rust-python-vectorbt.md`、`STEP.d/08-backtest.md`、`docs/backtest.md`：冻结规格、架构裁决与阶段文档。
- `config/config.example.toml`：补 `[backtest]` 全字段示例。
- `.gitignore`：忽略 `__pycache__/` 与 `*.pyc`。

### Fixed

- `tv:first-screen` / `tv:tf-switch:*` 曾在视窗应用前记录，测量到默认 227 根的中间态；`applyInitialViewport` 改为返回 Promise 并由调用方 await。
- 容器尺寸变化时 lightweight-charts 会把首屏 500 根重置为默认密度；`resize()` 现在保留变化前的逻辑区间。
- 回测权益在开盘价 mark-to-market：旧实现 `from_orders(targetpercent)` 只有一个价格序列，成交价与估值价都用开盘价，导致权益、回撤与收益偏差；现按收盘价重建权益，新增 `test_equity_is_marked_to_close_not_open` 回归测试。
- 对比面板 wrapper 与入口按钮重复使用 `id="bt-compare"`；wrapper 改为 `bt-compare-root`，按钮保留原 id，避免 DOM 查询命中错误节点。
