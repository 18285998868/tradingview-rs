# 行情数据口径

本文记录本地行情归档的目录布局、字段映射、时间口径、质量标记，以及数据层的
验收方法（AC-D01/D02/D06）。图表与回测都只读这些文件，不写原始归档。

## 一、目录布局

下载器 `data-downloader` 把 Binance 公共归档写到共享数据根：

```text
{root}/binance/data/{market}/{period}/{dataset}/{symbol}[/{interval}]/*.zip
```

- `market`：`spot`、`futures/um`、`futures/cm`；
- `period`：`daily` 或 `monthly`；
- `dataset`：`klines`、`aggTrades`、`trades` 等；
- K 线类多一层 `interval`（如 `1m`、`1h`）。

月度归档只包含完整月份；当月与最近几天由日档补齐。每个 zip 是一个同名 CSV，
上游另发布 `.CHECKSUM`；下载器在下载时流式校验 sha256，落盘只保留 zip，
重复执行按已有文件跳过。

本仓库使用 Binance 现货 12 个原生周期：

```text
1m 3m 5m 15m 30m 1h 2h 4h 6h 8h 12h 1d
```

## 二、字段映射

现货 K 线归档是无表头的 12 列 CSV，期货归档带表头，读取器两者都接受：

| 源列 | 字段 | 类型与处理 |
|---|---|---|
| 0 | `open_time` | UTC 毫秒；源值 ≥ 1e15 时视为微秒并整除 1000 |
| 1 | `open` | 十进制 → f64 |
| 2 | `high` | 同上 |
| 3 | `low` | 同上 |
| 4 | `close` | 同上 |
| 5 | `volume` | 同上，必须非负 |
| 6 | `close_time` | UTC 毫秒，同样做微秒归一 |
| 7 | `quote_volume` | 十进制 → f64 |
| 8 | `count` | 整数 |
| 9 | `taker_buy_volume` | 十进制 → f64 |
| 10 | `taker_buy_quote_volume` | 十进制 → f64 |
| 11 | `ignore` | 整数，归档占位字段 |

## 三、时间口径

1. 所有时间戳以 UTC 毫秒存储与传输；页面按浏览器本地时区渲染。
2. Binance 现货归档自 2025-01-01 起把 `open_time` / `close_time` 改为微秒。
   两条读取路径都按数量级归一到毫秒：Rust 的 `binance_core` 与 Python
   worker 的 `_read_archive`。
3. `close_time` 是 bar 的最后一毫秒：`open_time + 周期 - 1`（1d 为
   `open_time + 86_399_999`）。
4. `open_time` 必须按周期对齐；1m/1h/1d 等固定周期直接取模校验，`1mo` 走自然月。
5. 日档与月档重叠的日期以日档为准，读取时先读月档再按日档覆盖，避免重复。

## 四、数据质量标记

`GET /api/quality`（参数同 `/api/klines`）输出整段归档的只读诊断：

| 字段 | 含义 |
|---|---|
| `bar_count` / `expected_bars` | 实际 bar 数与首尾之间应有 bar 数 |
| `missing_bars` / `coverage` | 缺失数与覆盖率 |
| `gap_count` / `longest_gap_bars` / `gaps[]` | 缺口段数、最长缺口与明细（最多 20 条） |
| `zero_volume_bars` / `negative_volume_bars` | 零成交与负成交量 bar 数 |
| `bad_ohlc_bars` / `non_positive_price_bars` | OHLC 包络异常与非正价格 |
| `duplicate_rows` / `non_finite_rows` | 读取层兜底清洗丢弃的行数 |

读取器默认 `BadRowPolicy::Fail`，重复时间戳、非正价格、非法 OHLC、负成交量和
`close_time` 不匹配会直接报错，而不是静默丢弃。因此上述异常计数对当前数据源
通常为 0，它们是数据源或校验策略变化时的观测位。缺口口径与自然月处理见
[`../STEP.d/07-quality-report.md`](../STEP.d/07-quality-report.md)。

## 五、导入与校验

```bash
# 下载器：按配置的标的与周期补齐归档，已存在的文件跳过
/data/data-downloader/target/release/binance-data-downloader -c config.toml

# 已有归档的完整性校验（只读中央目录）
/data/data-downloader/target/release/binance-data-verify --dir /data/data/binance/data/spot/monthly/klines
```

配置字段与日期语义见 data-downloader 的 `docs/binance/config.md`。起始日留空时
按每个 job 的归档首日解析；解析走 `api.binance.com`，归档下载走
`data.binance.vision`，需要代理时可用 `HTTPS_PROXY` 加
`NO_PROXY=data.binance.vision` 分流。

## 六、验收方法

三个脚本都是标准库实现（AC-D01 额外复用回测 worker 的 pandas 加载器），
结果写入 JSON，失败时退出码 1。

| 项 | 脚本 | 做法 |
|---|---|---|
| AC-D01 | `scripts/data_official_check.py` | 12 个周期各取 1000 根固定窗口，本地加载器对 Binance 现货 REST，OHLC 逐值相等、成交量相对误差 ≤ 0.1% |
| AC-D02 | `scripts/source_field_check.py` | 12 个周期 × 2024-01/2025-01 月档，`binance-core` 解析结果与 zip 内 CSV 原文逐行逐字段比较，另校验时间对齐与 `close_time` |
| AC-D06 | `scripts/data_import_bench.py` | 按 24 小时成交额取前 100 个 USDT 现货标的，下载 12 周期月档并计时，统计文件数、字节数与失败数 |

AC-D06 的固定数据集是“100 标的 × 各自完整归档历史（最长 9 年）× 12 周期”，
不含日档与其他数据集；存储口径为全部 zip 字节，下载器校验后不保留 sidecar。
复跑会按已有文件跳过，测冷启动耗时需要换空目录。

## 七、实测结果

2026-11-16 在基准机（CachyOS，i5-12400F，32 GiB，NVMe）执行：

- AC-D01：BTCUSDT、ETHUSDT、SOLUSDT 各 12 个周期 × 1000 根，共 36,000 根，
  OHLC 差异 0，成交量最大相对误差 0；
- AC-D02：12 个周期 × 2024-01/2025-01 共 149,048 行，逐字段差异 0，缺口 0；
- AC-D06：100 标的 × 12 周期月档，49,620 个文件、10.82 GB、864 秒，0 个 404
  与失败，`binance-data-verify` 复核全部可读。

AC-D06 的 † 阈值按 9 年上界（133,200 个文件、约 34 GB、约 39 分钟）加余量
冻结为：全量导入 ≤ 60 分钟、存储 ≤ 50 GB。逐项方法与边界见
[`./validation.md`](./validation.md) 的“第零层 数据”。

边界：AC-D01 的官方对照取 2024-01-01 起的窗口，只覆盖三个现货标的；AC-D02
只抽两个月档，不是全库扫描；AC-D06 的耗时为单机、直连归档域名、并发 64 的
结果，换网络或并发会变化。
