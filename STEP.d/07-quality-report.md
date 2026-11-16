# 数据质量报告（/api/quality）

## 目标

为本地行情归档提供一个只读诊断端点：不打开图表也能知道某交易对/周期的时间范围、bar 数、缺口、零成交和 OHLC 异常。复用现有 `DataLoader` 缓存，不新增依赖，不改 SQLite schema；前端只新增一个只读徽标，见下文。

## 接口

```text
GET /api/quality?market_type=futures&pair=BTCUSDT&timeframe=1h
```

参数与 `/api/klines` 一致：`market_type` 缺省 `futures`，`pair` / `timeframe` 做字符集与长度白名单校验。响应仍是统一 `ApiResponse`，`data` 为报告对象。

## 字段

| 字段 | 含义 |
| --- | --- |
| `market_type` / `pair` / `timeframe` | 回显请求 |
| `bar_count` | 实际 bar 数 |
| `first_time` / `last_time` | 首尾 bar 开盘时间（毫秒） |
| `expected_bars` | 首尾之间应有的 bar 数，含端点 |
| `missing_bars` | `expected_bars - bar_count` |
| `coverage` | `bar_count / expected_bars`，保留 6 位小数 |
| `gap_count` | 连续缺失段数 |
| `longest_gap_bars` | 单段最大缺失 bar 数 |
| `gaps[]` | 缺口明细 `{after, before, missing_bars}`，最多 20 条 |
| `gaps_truncated` | 明细是否被截断 |
| `zero_volume_bars` / `negative_volume_bars` | 成交量为 0 / 负的 bar 数 |
| `bad_ohlc_bars` | `high < low`、`high < open/close`、`low > open/close` 的 bar 数 |
| `non_positive_price_bars` | 任一 OHLC 价格 ≤ 0 的 bar 数 |
| `duplicate_rows` / `non_finite_rows` | 本层清洗丢弃的重复时间戳 / 非有限值行数 |

## 缺口口径

- 缺口只在相邻两根已有 bar 之间计算，首尾之外不推测。
- binance 用 `Interval::step_ms()`；`1mo` 没有固定毫秒，走 `Interval::next_open()` 的自然月推进，2 月、闰年和 28/31 天月份都能对齐。
- dukascopy 用 `Timeframe::interval_ms()`。本地归档的周末 bar 是平盘零成交，不是缺 bar，因此固定步长不会把周末误判成缺口。
- 非整倍步长按 `(diff - 1) / step` 计入缺失；小于一个步长的间隔不计。
- `expected_bars` 由 `bar_count + 各缺口缺失数` 累加得到，保证 `missing_bars` 与缺口明细一致。

## 统计层与 reader 校验的关系

binance reader 默认 `BadRowPolicy::Fail`，会直接拒绝重复时间戳、非正价格、非法 OHLC、负成交量和 close_time 不匹配的行；dukascopy decoder 也会丢弃非法 OHLC。故 `bad_ohlc_bars` / `non_positive_price_bars` / `negative_volume_bars` 对当前数据源通常是 0，它们是对 reader 行为变化的兜底观测：

- 若以后放宽 reader 校验或接入新数据源，这些计数会非零。
- `duplicate_rows` / `non_finite_rows` 来自 `KlineColumns::from_parts` 的兜底清洗计数，不依赖 reader 报告。
- `zero_volume_bars` 是真实业务信号：dukascopy 周末会大量出现，binance 偶尔出现。

## 实现

- `src/app/quality.rs`：报告 DTO、`BarStep`（固定毫秒 / 自然月）与 `analyze` 纯函数。
- `src/app/data_loader.rs`：`KlineColumns` 增加 `clean: CleanStats` 与 `clean_stats()`；`DataLoader::quality_report` 先解析步长，再复用 `load_all_klines` 的缓存读盘，最后调用 `analyze`。
- `src/app/routes.rs`：`QualityQuery` 与 `quality_handler`，读盘走 `spawn_blocking`，错误映射沿用 `handle_data_error`（400 / 404 / 500）。
- 报告是整段列集的统计；数据量大时和 `/api/klines` 全量请求一样会占用缓存，不额外复制列集。

## 前端徽标

- `frontend/src/quality.ts` 的 `qualityBadge()` 是纯函数：覆盖率与异常决定等级，零成交单独出现不降级；`formatQualityPercent()` 负责百分比格式（最多两位小数、去尾零）。
- 信息栏 `#quality-badge` 默认 `hidden`；`loadQuality()` 在整段 K 线加载完成后异步拉取，`invalidateQualityBadge()` 递增 `qualitySeq` 并清空文本/提示，供 `teardownChart`、全量加载分支与三个切换入口复用。
- 过期保护有两层：序号不匹配直接丢弃响应，另外再比对 `market_type` / `pair` / `timeframe` 与当前选择一致。
- 徽标反映整段归档，不受 200 根分页窗口影响；后端返回的清洗计数对当前数据源通常是 0，接到数据源放宽校验后会直接显示出来。
- 测试：`frontend/scripts/quality.test.mjs` 覆盖绿/黄/红阈值、零成交不降级、百分比格式，`cd frontend && npm test` 运行。

## 验证

- `quality.rs` 单测：固定步长相邻/缺口/非整倍间隔、自然月跨 2 月、缺口与异常统计、清洗计数与缺口明细上限。
- `data_loader.rs` 单测：`from_parts` 的 NaN/重复计数。
- `tests/integration_test.rs`：真实 zip fixture 的 `quality_report` 字段、非法周期 400。
- `routes.rs` 单测：非法 pair/timeframe/market_type 400，无数据 404。
- 前端：`npm test` 覆盖徽标纯函数；`tsc --noEmit` 与 `vite build` 通过后，用 geckodriver + headless Firefox 冒烟，校验徽标文本、`title` 与切换标的后旧徽标立即消失。
- 发布前在远程执行完整 cargo 门禁，并用真实数据冒烟 `futures/BTCUSDT/1h` 与 `dukascopy/EURUSD/H1`。
