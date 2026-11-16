# tradingview

## 项目目标

> TradingView 风格本地研究终端：读本地 binance zip（futures/spot）与 dukascopy bi5（外汇/贵金属），经 REST 提供给前端图表，用 vectorbt 跑 Binance 现货回测，并把布局、绘图、偏好与回测结果持久化到本地 SQLite。
>
> - 后端（`tradingview-backend`）：axum 服务，行情 GET 路由 + `/api/layouts|drawings|preferences` 状态路由，binance/dukascopy reader 读数据 + moka 缓存，前端静态目录同服。
> - 回测（`python/backtest_worker`）：Python + vectorbt 1.2.0 计算双均线只做多，Rust 负责 `/api/backtests*`、任务状态、持久化、回撤分析与对比导出。
> - 前端（`tradingview-frontend`）：vite + lightweight-charts 5 原生 pane，绘图用 lightweight-charts-drawing，dev 经 proxy 调 `/api`。
> - 状态库：SQLite（rusqlite bundled），默认 `<data_dir>/tradingview.db`，只存 UI/研究状态、回测任务与回测结果，不存行情。

## 技术选型

- rust（后端）/ TypeScript（前端）
- edition = "2024"，工具链由 `rust-toolchain.toml` 固定为 `channel = "stable"`（永远用最新 stable，与仓库惯例一致；本机默认的 1.88 低于依赖链要求，裸 `cargo build` 会失败）
- HTTP：axum 0.8 + tower-http 0.7（cors/fs/compression-gzip）
- 数据：binance-core 0.2.0 + dukascopy-core 0.2.0（path 依赖 data-reader workspace，分别读 binance zip / dukascopy bi5）+ moka 0.12（sync 缓存）
- 回测：Python 3.14 + vectorbt 1.2.0 + pandas/numpy；Rust 以子进程调用，JSON 文件交换
- 前端：vite 6 + lightweight-charts 5.2 + lightweight-charts-drawing 0.6 + technicalindicators 3
- 状态库：rusqlite 0.41（bundled SQLite）+ uuid 1（布局与回测运行 ID）
- 配置：`config/config.toml` + serde（derive）
- 错误：thiserror 2（库层 `AppError`）+ anyhow 1（仅入口收敛）
- 运行时：tokio 只开 `macros / rt-multi-thread / net / fs / io-util / signal / process / time`

## 冒烟配置

```toml
# config/config.toml
host = "127.0.0.1"           # 可选，默认 127.0.0.1
port = 3001                  # 可选，默认 3001
data_dir = "path/to/data"   # 必填，无默认，须指向数据根
# db_path = "path/to/data/tradingview.db"  # 可选，默认 <data_dir>/tradingview.db
# frontend_dir = "<workspace>/frontend/dist"  # 可选，默认 CARGO_MANIFEST_DIR/frontend/dist
# cors_origins = ["http://localhost:5173"]   # 可选，默认 [] = 全放行（仅本地开发）
# auth_token = "change-me"                   # 可选；启用 Bearer/Basic 认证，非回环 host 必填
# max_body_bytes = 2097152                   # 可选，默认 2 MiB，范围 1 KiB 至 64 MiB

[backtest]                    # 可选；缺省 enabled=true
python_bin = "/data/vbt/.venv/bin/python"  # 必须是装有 vectorbt 1.2.0 的解释器
worker_dir = "python"         # 含 backtest_worker/ 的目录
# enabled = true
# output_dir = "<data_dir>/backtests"
# timeout_secs = 1800
# max_concurrent = 2
# max_compare_runs = 10
# max_grid_runs = 100
# max_curve_points = 2000
# max_trades = 200000
# keep_equity = true
```

**路径与日志的优先级**（都是"显式 > 通用 > 默认"）：

| 项 | 优先级 |
|---|---|
| 配置文件 | `TRADINGVIEW_CONFIG` > `./config/config.toml`（相对 CWD） > `<crate 根>/config/config.toml`（编译期绝对路径，只在开发机有效） |
| 日志过滤 | `TRADINGVIEW_LOG` > `RUST_LOG` > `tradingview_backend=info,tower_http=info` |
| `.env` | **只读 CWD 下的 `.env`**（不向上遍历目录树，且不覆盖已有环境变量） |

> 早先用 `dotenvy::dotenv()` 会一路往上找 `.env`，仓库根的 `RUST_LOG=debug` 因此无声生效，服务自己的默认过滤规则形同虚设。

## Docker Compose

需要并排的 `data-reader` 仓库、Docker Compose v2.20+ 与 buildx：

```bash
cp .env.example .env   # 写入 TV_AUTH_TOKEN，例如 openssl rand -hex 32
docker compose up -d --build --wait
```

默认映射宿主机 3001，状态保存在命名卷。变量、挂载、构建代理与升级方式见
[`./docs/backtest.md#docker-compose-部署`](./docs/backtest.md#docker-compose-部署)。

## 逻辑编排

```text
启动 -> main.rs: 读 CWD/.env → tracing_subscriber::fmt(log_filter())
  → Config::load() → run(&config)
v
run(&config):
  1. data_dir 不存在直接 AppError::Config；frontend dist 缺失只 warn（纯 API 模式）
  2. 打开 StateStore（SQLite，WAL）→ AppState::new(data_dir, state_store)
     （DataLoader + moka cache + 状态库）
  3. create_router() → /api/* 路由 + CORS（cors_origins 为空则全放行 + warn）+ ServeDir(frontend/dist)
  4. TcpListener::bind(host:port) + axum::serve(...).with_graceful_shutdown(SIGINT/SIGTERM)
v
请求 -> Router -> Handler(Query+State) -> spawn_blocking{ DataLoader / SQLite } -> ApiResponse{success, data, error} Json
```

## 架构设计

```text
src/main.rs         # 入口：dotenv(CWD) → tracing → Config::load() → run(&config)；anyhow 收敛
src/lib.rs          # 只做导出 + crate 级 lint + README 内联文档
src/app/mod.rs      # 执行代码：run(config) 编排（data_dir/状态库/AppState/路由+CORS/静态/serve）+ 优雅停机
src/app/routes.rs   # 执行代码：AppState（DataLoader + 状态库 + 认证/body 上限）+ ApiResponse + 行情/状态路由
src/app/auth.rs     # 支撑件：Bearer/Basic 认证中间件（常量时间比较）
src/app/data_loader.rs # MarketType{Futures,Spot,Dukascopy} + KlineColumns(SoA) + DataLoader：两个 reader + moka 两级缓存（列集按字节计权、目录扫描 60s）；发现层按 reader 约定：binance daily∪monthly、dukascopy 文件名粒度
src/app/state.rs    # 状态库：layouts/drawings/preferences + backtest_runs 等 CRUD + 限额校验 + SQLite 连接
src/app/backtest/   # 回测：model 契约 / store SQLite / service 子进程与状态机 / compare 对比与 CSV / routes
src/app/udf/        # TradingView UDF：symbol/resolution/history/price/protocol/handlers 六个子模块
python/backtest_worker/ # 数据读取、双均线 vectorbt 引擎、回撤/指标/降采样、CLI
migrations/         # SQLite 表结构（include_str! 进二进制，启动时幂等建表）
src/config.rs       # 支撑件：Config{host/port/data_dir/db_path/frontend_dir/cors_origins/auth_token/max_body_bytes/backtest} + load/load_from + validate
src/error.rs        # 支撑件：AppError(Io/Config/ConfigRead/ConfigParse/Data/State/InvalidAddr, non_exhaustive)
src/utils/mod.rs    # 支撑件：valid_pair/valid_timeframe/has_kline_files/has_data_files/timeframe_order
frontend/           # 前端工程：src/{api,app,backtest,chart,indicators,main,quality,types,viewport} + vite.config(dev 5173, proxy /api→3001)
```

## 实现

### main.rs / lib.rs

- `main` 只做三件事：日志初始化 → `Config::load()` → `run(&config)`；错误用 `anyhow::Context` 补上下文。
- `run` 先校验 `data_dir.is_dir()`（否则直接 `Config` 错）；`frontend_dir` 缺失降级纯 API 模式（warn 不中断）。
- 库层错误统一走 `AppError`（`thiserror`），**不提供 `From<anyhow::Error>`**：`anyhow::Error` 不是 `std::error::Error`，压进枚举会丢 source 链，也会把任意失败误标成"配置错误"。

### config

解析层：

1. 路径优先级见上表；TOML 可解析，失败带路径（`AppError::ConfigParse { path, source }`）。

语义层：`host/port/frontend_dir` 有缺省；`data_dir` 必填；`db_path` 缺省为 `<data_dir>/tradingview.db`，显式给出时不能为空；`cors_origins` 缺省为空（= 全放行）。路径字段一律 `PathBuf`，不用 `String` 装路径。

### routes & data_loader

- `GET /api/health` 健康检查；`/api/market-types|pairs|timeframes|klines|quality` 按 `market_type` + `pair` + `timeframe` 取数或统计；`/api/backtests*` 见下节。
- **`/api/market-types` 只列"本地确有数据源"的类型**：`futures`（binance um）+ `dukascopy`（外汇/贵金属）。`spot` 变体仍可解析，但行情接口不对外暴露（现货 zip 只由回测 worker 直接读取）—— 否则前端会列出与图表数据源不一致的选项。
- 入参做目录穿越防护（`..`/`/etc/passwd` 类输入有单测覆盖），并限长（pair ≤ 32、timeframe ≤ 16）。
- 请求规模防护：`limit` ≤ 10000，超限返回 400。
- **周期合法性由数据源裁定**：binance 支持 `1m..1mo`，dukascopy 支持 `M1/H1/D1/1m/1h/1d`。输错返回 400（`DataError::InvalidParam`），并回传该源支持的周期列表。
- **发现层按 reader 的归档约定走，不假设单一目录**：binance 取 `daily ∪ monthly`（reader 合并两处，日线优先），否则只有月度归档的下架品种（如 `FTTUSDT`/`LUNAUSDT`）和大周期（`3d`/`1w`/`1mo`）会整体消失；dukascopy 按文件名粒度（`BID_candles_{min_1|hour_1|day_1}.bi5`）**只列真实存在的周期**，避免默认落到没下过的粒度。
- `DataLoader` 按市场类型定位数据文件，读后进 moka 缓存，统一包 `ApiResponse{success, data, error}`。
- 状态路由：`/api/layouts`（列表/创建）、`/api/layouts/{id}`（读取/更新/删除）、`/api/drawings?market_type=&pair=`（读取/整组替换）、`/api/preferences`（读取/合并写入）。状态错误按输入 400、内部 500 映射，404 只用于不存在的布局。
- 缓存用 **SoA 列集** `KlineColumns`（`time/open/high/low/close/volume` 六条 `Vec`）：reader 本身就是列存储，中间层再存 `Vec<Kline>` 会多出 6 份结构体拷贝。
- 缓存预算按字节而不是条数：权重取六列 `Vec::capacity` 之和（约 48 字节/bar），上限 2 GiB；条目空闲 10 分钟回收，最长存活 30 分钟（归档由外部下载器更新，TTL 给"新数据最迟多久可见"一个上界）。同一 key 的并发未命中由 moka `try_get_with` 收敛成一次读盘，其余请求等它的结果。
- 目录扫描（`list_pairs` / `list_timeframes`）另有一级 60 秒 TTL 缓存，键分别是市场、市场 + 交易对；否则 `/api/pairs`、`/udf/search`、`/udf/symbols` 每次请求都要递归 `read_dir` 并逐个品种探测数据文件。
- 响应压缩：客户端带 `Accept-Encoding: gzip` 时由 tower-http 压缩（`Fastest` 级别，只压 >32 字节且非图片/SSE/gRPC 的响应）。实测 1m 全量 4.3 MB → 1.3 MB，release 下多花约 5–10 ms CPU；没有用默认压缩级别，因为本地回环带宽不是瓶颈，默认级别的 CPU 开销换不回相应的传输收益。
- 时间过滤用二分（`partition_point`）定位窗口，只拷贝窗口内的 bar；`limit` 取窗口内**最后** N 根。
- `GET /api/quality` 复用同一份缓存列集，输出 bar 数、首尾时间、按周期步长推算的应有 bar 数/缺失数/覆盖率、缺口段数与明细（最多 20 条）、零/负成交量、OHLC 包络异常、非正价格，以及清洗阶段丢弃的重复时间戳/非有限值行数。缺口按数据源周期步长计算，binance `1mo` 按自然月；报告只读缓存，不写状态库。

### 网络边界与认证

- 默认 `host = "127.0.0.1"`，只监听回环。配置 `auth_token` 后，`/api/*` 与 `/udf/*` 都要求 `Authorization: Bearer <token>`，或浏览器 Basic 登录（用户名任意，密码为令牌）。
- `/api/health` 与 `OPTIONS` 预检豁免；静态页面不拦，否则浏览器无法弹出 Basic 登录框。401 带 `WWW-Authenticate: Basic realm="tradingview"`。
- 非回环 `host` 未配置令牌时启动失败（`auth_token is required when host is not loopback`）；令牌为空、含空白或控制字符也会被拒绝。认证是单令牌、无会话、无多用户，不内置 TLS；公网部署仍需反向代理终止 TLS，并优先使用 Bearer 或 HTTPS。
- 请求体上限 `max_body_bytes` 直接传给 `DefaultBodyLimit`，默认 2 MiB，合法范围 1 KiB 至 64 MiB。

### 并发模型（阻塞 IO 不占 worker）

tokio 的 worker 线程只做调度。读 zip（实测 52ms）这类同步重活必须挪到 blocking 线程池，否则并发几个请求就能占满 12 个 worker，轻请求（`/api/health`）也一起卡死。

- `/api/pairs`、`/api/timeframes`、`/api/klines`、`/api/quality`：`spawn_blocking` 包裹数据层调用。
- `/api/layouts|drawings|preferences`：所有 SQLite 操作同样走 `spawn_blocking`；连接由 `Mutex<Connection>` 串行化（本地单用户，写少读多）。
- blocking 线程里的 panic 会被 tokio 捕获成 `JoinError`，统一收敛成 500，不会变成进程级故障。

### 回测（Backtest）

- 范围：Binance 现货、只做多、12 个原生周期；策略为双均线 `dual_ma`，默认 `fast=10, slow=30`。
- 数据：worker 直接读 `{data_dir}/binance/data/spot/{daily,monthly}/klines`，不经过 `/api/market-types`；`spot` 仍不在行情接口对外暴露。
- 计算：Rust 写请求 JSON → 子进程 `python -m backtest_worker.cli` → vectorbt 1.2.0 计算 → Rust 读取结果并落库；Python 只读现货 zip，不依赖 `libbinance_core.so`。
- 接口：`/api/backtests` 提供 config/提交/列表/详情/日志/重试/删除/对比/CSV/命名对比；参数扫描共享 `batch_id`，每组参数一条运行，上限由 `max_grid_runs` 控制。
- 状态：`queued → running → succeeded|failed`；服务重启把遗留的运行标记失败，重试创建新 ID 并保留历史。
- 保留：指标、回撤事件与交易全量（交易按 `max_trades` 截断）；水下曲线始终降采样保留，权益曲线受 `keep_equity` 控制。
- 对比：2 到 10 个运行，按时间戳并集对齐，缺失段写 `null`；标的/周期/策略版本/数据区间/成本不一致时警告，参数不同不警告但必须展示；对比可命名保存、列表找回与删除，运行缺失时以 `missing_runs` 标出。
- 配置、请求示例、失败语义与验证命令见 [`./docs/backtest.md`](./docs/backtest.md)。

### 回放（Replay）

- 前端维护两份数据：`allKlines` 是本次回放加载的完整窗口（当前取数上限 2000 根，以进入回放时的最后一根为终点），图表只接收 `allKlines[0..=cursor]` 的前缀。这样指标的输入始终截止当前游标，不会提前看到未来 bar。
- 进入回放时以**当前视口左端**为起点；若该时间落在窗口内，就从对应 bar 开始。重置回到同一游标，退出回放恢复完整窗口和进入前的视口。
- 控件：播放/暂停、单步、0.5×/1×/2×/4×/8×、重置、退出。播放到最后一根自动暂停。
- 图表层 `updateData()` / `appendData()` 在“前缀不变、只追加或改写最后一根”时走 `series.update()` 写尾部，不再整段 `setData`；`prependData()` 受 lightweight-charts 限制仍整段重绘，但只转换新增的前缀。指标 series 在结构不变时复用已有 series 与 pane，指标序列同样只更新尾部。
- 回放游标属于临时交互状态，不写入 SQLite；`last_session` 仍只保存交易对/周期/指标/视口。

### 指标 parity

- 26 个指标以 Pine/TradingView 的“首有效 bar + null 暖机”为口径；未暖机位置一律 `null`，不会画成 0。
- 修正了对齐偏差：PSAR、Ichimoku、Chandelier、ADX、MFI、KST、TRIX、OBV、AO，以及 MACD/Stoch 的暖机段。
- Ichimoku 改为自实现：转换线/基准线画在当前 bar，先行 A/B 右移 `displacement` 根；不再直接采用库的“四线同起点”输出。
- 回归测试：`cd frontend && npm test`。覆盖首有效下标、无 `undefined`、暖机后无空洞、MACD 三线同轴、Ichimoku 位移、振荡指标值域，以及“完整序列 vs 回放前缀”在重叠区间逐点一致；另有图表增量判定的纯函数测试（尾部扩展、最后一根改写、乱序回退、指标前缀兼容）。

### 数据质量徽标

- 信息栏徽标由 `/api/quality` 驱动：一段 K 线加载完成后异步拉取，不阻塞图表；报告按整段归档统计，与图表当前只加载的 200 根窗口无关。
- 等级只看覆盖率与异常：覆盖率 ≥ 99.9% 且无缺口、无异常为绿色；有缺口或覆盖率 < 99.9% 为黄色；覆盖率 < 99% 或任一 OHLC / 价格 / 成交量 / 清洗异常为红色。零成交量是业务信号（例如外汇周末），单独出现仍保持绿色。
- 徽标文本形如 `质量 99.9% · 缺口 1根`，悬停提示给出 bar 数/应有 bar 数、缺口段数与最长缺口、零/负成交量、OHLC 与清洗计数。
- 切换交易对、市场或周期时先递增请求序号并立即隐藏旧徽标，飞行中的旧响应按序号丢弃，避免旧报告贴到新标的上。
- 纯函数在 `frontend/src/quality.ts`，测试在 `frontend/scripts/quality.test.mjs`（`cd frontend && npm test`）。

### UDF 数据源

- 独立的 `/udf/*` 命名空间，复用本地 `DataLoader`，不改变 `/api/*` 契约。Charting Library 的 Datafeed URL 填 `http://127.0.0.1:3011/udf` 即可。
- 端点：`GET /udf/config`、`/udf/time`、`/udf/symbols?symbol=`、`/udf/search?query=&type=&exchange=&limit=`、`/udf/history?symbol=&resolution=&from=&to=&countback=`。
- symbol：`BINANCE:BTCUSDT`（binance UM futures）或 `DUKASCOPY:EURUSD`；裸交易对默认按 futures 处理。`Spot` 归档只供回测 worker 直接读取，行情与 UDF 接口不暴露。
- resolution：UDF 分钟数映射到本地周期，例如 `1→1m`、`60→1h`、`240→4h`、`1D→1d`、`3D→3d`、`1W→1w`、`1M→1mo`；dukascopy 对应 `1→M1`、`60→H1`、`1D→D1`。`/udf/symbols` 会按该交易对实际存在的周期返回 `supported_resolutions`，有周/月数据时同时给出 `has_weekly_and_monthly` 与 `weekly_multipliers`/`monthly_multipliers`。
- `/udf/history` 的 `from` 含、`to` **不含**，均为秒级；返回 `{s:"ok",t,o,h,l,c,v}`，其中 `t` 也是秒。`countback` 优先于 `from`（出现即忽略 `from`，从 `to` 往回数）。窗口为空返回 `{s:"no_data"}`，若过去有数据则带 `nextTime`；非法 symbol/resolution 返回 `{s:"error",errmsg}`。
- `pricescale` 由该交易对最粗周期的最近 200 根 OHLC 小数位推导，静态值只作下限；`minmov` 恒为 1，即 tick = `1/pricescale`。
- `session`：crypto 为 `24x7`；dukascopy 为 `0000-0000`（周一到周五，`Etc/UTC`）。
- 单次最多 10000 根；`countback` 用于只要最近 N 根（`from/to` 可为 0）。时间过滤仍在 blocking 线程池执行，读盘结果走现有 moka 缓存。
- 代码结构：`src/app/udf/{symbol,resolution,history,price,protocol,handlers}.rs`，handler 只做参数解析与响应映射，读盘全部走 `spawn_blocking`。
- 回归测试：`cargo test -p tradingview-backend --test udf_test`，覆盖五个端点、秒级时间、`to` 右开边界、`no_data` 与非法 resolution；`--test udf_contract_test` 逐字段锁定 JSON 契约；官方语义依据见 `./STEP.d/02-udf-contract.md`。
- 重构范围与协议修复清单见 `./STEP.md`。

状态数据与行情文件分离：行情仍然只读 zip/bi5，SQLite 只存研究终端的 UI 状态与回测结果。默认路径 `<data_dir>/tradingview.db`，启动时幂等执行 `migrations/001_init.sql` 与 `migrations/002_backtest.sql`，并开启 WAL。

- `layouts`：命名布局，一行一份**完整会话快照**（`market_type/pair/timeframe/indicators/viewport`）。名称唯一，列表只回摘要，读取详情才回 `payload`。
- `drawings`：按 `(market_type, pair)` 一行存整组绘图（`lightweight-charts-drawing` 导出的 JSON 数组）。跨周期共享；`PUT` 是整组替换，不是增量合并。
- `preferences`：KV 合并写入。`last_session` 存最近一次会话，用于启动恢复。
- 限额：布局 payload ≤ 1 MiB；绘图 ≤ 500 个/对、单图 ≤ 512 锚点、整组 ≤ 1 MiB；偏好 ≤ 128 键 / 256 KiB；HTTP body 默认 ≤ 2 MiB，由 `max_body_bytes` 配置（1 KiB 至 64 MiB）。
- 前端自动保存：会话 800ms debounce，绘图 600ms debounce；恢复/加载布局期间用 `suppressSave` 抑制回写，绘图在服务端加载完成前用 `drawingsLoaded` 阻止空画布覆盖。

## 问题

- 当 `data_dir` 指向不存在目录时，
  - 解决：启动即校验，`AppError::Config` fast-fail
  - 文件：`src/app/mod.rs`
- 当只想要 API 不要前端时，
  - 解决：`frontend_dir` 缺失仅 warn，服务以降级纯 API 模式继续
  - 文件：`src/app/mod.rs`
- 当生产环境需要收紧跨域时，
  - 解决：`config.cors_origins` 非空时只放行列表内 origin；为空则全放行 + warn
  - 文件：`src/app/mod.rs`、`src/config.rs`
- 当需要从其他主机访问 API 时，
  - 解决：必须配置 `auth_token`，启用 Bearer/Basic 认证；非回环监听未配令牌直接启动失败
  - 文件：`src/app/auth.rs`、`src/config.rs`
- 当请求试图穿越目录时，
  - 解决：入参校验，单测覆盖 `..` 与绝对路径攻击串
  - 文件：`src/app/routes.rs`（校验逻辑在 `src/utils/mod.rs`）
- 当默认工具链过旧导致构建失败时，
  - 解决：`rust-toolchain.toml` 固定 `channel = "stable"`（进目录即自动切到最新 stable）
  - 文件：`rust-toolchain.toml`
- 当进程被信号杀死截断在途请求时，
  - 解决：`with_graceful_shutdown` 等在途请求跑完
  - 文件：`src/app/mod.rs`
- 当刷新页面丢失周期/指标/绘图时，
  - 解决：`last_session` + 按交易对绘图组持久化到 SQLite，启动恢复；命名布局可显式保存多套
  - 文件：`frontend/src/app.ts`、`src/app/state.rs`、`migrations/001_init.sql`
- 当回测 worker 失败、超时或服务重启时，
  - 解决：worker 级失败整批标 `failed` 并保留日志；服务重启把遗留 `queued`/`running` 标 `failed`；重试创建新运行 ID
  - 文件：`src/app/backtest/service.rs`
- 当对比运行数量超限或元数据不一致时，
  - 解决：少于 2 或多于 10 直接 400，不静默截断；标的/周期/策略版本/数据区间/成本不一致时警告，参数差异只展示
  - 文件：`src/app/backtest/compare.rs`
- 当交易对/周期只存在于月度归档时（下架合约、`3d`/`1w`/`1mo`），
  - 解决：发现层扫 `daily ∪ monthly` 并去重，reader 负责按日合并
  - 文件：`src/app/data_loader.rs`
- 当 dukascopy 只下了一部分粒度时，
  - 解决：按 `BID/ASK_candles_{min_1|hour_1|day_1}.bi5` 文件名核对，只列真实存在的周期
  - 文件：`src/app/data_loader.rs`

## 完整架构

```text
src/main.rs         # 入口：dotenv(CWD) → tracing → Config::load → run
src/lib.rs          # 只做导出 + crate 级 lint + README 内联文档
src/app/mod.rs      # run(config)：校验/状态/路由/CORS/静态/serve/优雅停机
src/app/routes.rs   # AppState(DataLoader + 状态库 + 认证/body 上限) + ApiResponse + 行情/状态路由 + 穿越防护
src/app/auth.rs     # Bearer/Basic 认证中间件（常量时间比较）
src/app/data_loader.rs # MarketType + KlineColumns(SoA) + DataLoader：两个 reader + moka
src/app/state.rs    # StateStore：layouts/drawings/preferences + 回测运行 CRUD + 限额校验 + SQLite
src/app/backtest/   # 回测：model 契约/store SQLite/service 子进程与状态机/compare 对比与 CSV/routes
src/app/udf/        # UDF：symbol/resolution/history/price/protocol/handlers
python/backtest_worker/ # 数据读取、双均线 vectorbt 引擎、回撤/指标/降采样、CLI
migrations/001_init.sql # layouts/drawings/preferences 表
migrations/002_backtest.sql # backtest_runs/trades/drawdown_events/equity/underwater 表
src/config.rs       # Config{host/port/data_dir/db_path/frontend_dir/cors_origins/auth_token/max_body_bytes/backtest} + load/load_from + validate
src/error.rs        # AppError(Io/Config/ConfigRead/ConfigParse/Data/State/InvalidAddr, non_exhaustive)
src/utils/mod.rs    # valid_pair/valid_timeframe/has_kline_files/has_data_files/timeframe_order
frontend/src/       # api/app/backtest/chart/indicators/main/quality/types/viewport + style.css + vite-env
```

## 完整配置

```toml
# config/config.toml
host = "127.0.0.1"           # 可选，默认 127.0.0.1，监听地址
port = 3001                  # 可选，默认 3001
data_dir = "path/to/data"   # 必填，无默认，数据根
db_path = "path/to/data/tradingview.db"  # 可选，默认 <data_dir>/tradingview.db
frontend_dir = "/data/tradingview-rs/frontend/dist"   # 可选，默认 <workspace>/frontend/dist
cors_origins = []           # 可选，默认 [] = 全放行（仅本地开发）
# auth_token = "change-me"  # 可选；启用 Bearer/Basic 认证，非回环 host 必填
# max_body_bytes = 2097152  # 可选，默认 2 MiB，范围 1 KiB 至 64 MiB

[backtest]                  # 可选；缺省 enabled=true
python_bin = "/data/vbt/.venv/bin/python"  # 必需：装有 vectorbt 1.2.0 的解释器
worker_dir = "python"       # 含 backtest_worker/ 的目录
# enabled = true
# output_dir = "<data_dir>/backtests"
# timeout_secs = 1800
# max_concurrent = 2
# max_compare_runs = 10
# max_grid_runs = 100
# max_curve_points = 2000
# max_trades = 200000
# keep_equity = true
```

- 全量示例见 `config/config.example.toml`。
- `[backtest]` 缺省启用；`enabled = false` 时 `/api/backtests*` 返回 503，行情接口不受影响。

## 完整执行逻辑

- src/main.rs
  - 入口
  - 读 CWD/.env → `tracing_subscriber::fmt`（`TRADINGVIEW_LOG` > `RUST_LOG` > `tradingview_backend=info`）→ `Config::load()` → `run(&config)`
- src/app/mod.rs
  - `run(&Config)` 唯一编排：校验 `data_dir` → 打开 `StateStore` → 建 `AppState` → 路由+CORS+静态 → `bind+serve`（优雅停机）
- src/config.rs
  - `load()`：按优先级定位配置文件 → `toml::from_str` → 缺省补齐 → `validate()`（非回环必须配 `auth_token`，body 上限区间校验）
- src/error.rs
  - `AppError`（thiserror, non_exhaustive）：Io/Config/ConfigRead/ConfigParse/Data/State/InvalidAddr
- src/app/routes.rs
  - 中间件：`DefaultBodyLimit`（`max_body_bytes`）+ Bearer/Basic 认证（`/api/health`、`OPTIONS`、静态资源豁免）
  - 行情 GET：health / market-types / pairs / timeframes / klines / quality
  - 状态：`/api/layouts`、`/api/layouts/{id}`、`/api/drawings`、`/api/preferences`
  - 回测：`/api/backtests`、`/api/backtests/{id}`、`/log`、`/retry`、`/compare`、`/compare.csv`、`/comparisons`
- src/app/state.rs
  - SQLite CRUD + 输入限额 + `spawn_blocking` 调用边界；布局/绘图/偏好 + 回测运行/交易/事件/曲线
- src/app/backtest/
  - `model.rs` 契约与参数网格 → `service.rs` 状态机、并发许可、子进程与超时 → `store.rs` 事务落库 → `compare.rs` 警告、时间并集与 CSV
- src/app/data_loader.rs
  - `MarketType` 解析 → 发现（binance `daily ∪ monthly`，dukascopy 按 `BID/ASK_candles_*` 文件名）→ reader 读 → 去非有限值/去重 → moka 存 → 返回列集
- src/utils/mod.rs
  - 参数白名单校验 + 数据文件判别 + 周期排序
- frontend/
  - `api.ts` 调 `/api/*` → `chart.ts`（lightweight-charts 5 单 chart + 原生 pane + DrawingManager）渲染 → `indicators.ts`（technicalindicators）叠加 → `backtest.ts` 回测面板与 SVG 曲线 → `app.ts` 负责会话/布局/绘图自动保存

## 命令

```bash
cargo fmt
cargo check -p tradingview-backend --all-targets
cargo clippy -p tradingview-backend --all-targets -- -D warnings
cargo test -p tradingview-backend
cargo run -p tradingview-backend                  # 读 config/config.toml，:3001 对外
cd frontend && npm install && npm run dev         # 前端 dev（:5173，/api 代理到 :3001）
cd frontend && npm run build                       # 产物进 frontend/dist，被后端同服；dist 已入库

# 回测 worker（解释器需装 vectorbt 1.2.0）
cd python && /data/vbt/.venv/bin/python -m unittest discover -s tests -v
```

回测真实数据冒烟与接口验证见 [`./docs/backtest.md`](./docs/backtest.md)，行情数据
口径与数据层验收见 [`./docs/data.md`](./docs/data.md)，四层验证结果见
[`./docs/validation.md`](./docs/validation.md)，服务冒烟脚本见 [`./STEP.md`](./STEP.md) 模块五。

> 工具链由 `rust-toolchain.toml` 固定为 `channel = "stable"`，上述命令无需手工指定版本号。
>
> `frontend/dist` 是入库产物：改 `frontend/src` 后必须重新 build 并提交 `dist`，否则后端默认静态目录仍提供旧包。

## 配置

见 `config/config.example.toml`（全字段）与 `config/config.toml`（当前生效）。
