# Changelog

## Unreleased

### Changed

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

### Added

- `tests/udf_contract_test.rs`：锁定 `/udf/config`、`/udf/symbols`、`/udf/search`、`/udf/history` 的 JSON 契约与官方语义（`to` 右开、`countback` 优先、`nextTime`、`pricescale` 采样、周线能力）。
- `STEP.md` 与 `STEP.d/`：拆分范围、协议修复清单与官方语义依据。
- 缓存测试：权重按字节换算、权重确实注册进缓存、8 线程并发未命中只读一次盘、错误分类往返。
- 目录缓存测试：扫描结果在 TTL 内复用、`invalidate_cache` 后立刻可见、按市场分键。
- `STEP.d/03-kline-cache.md`：缓存硬化的设计取舍与验证命令。
- `STEP.d/04-catalog-cache.md`：目录扫描缓存的设计与验证。
- `STEP.d/05-compression.md`：gzip 级别取舍与 ETag 决策。
- `tests/http_compression_test.rs`：gzip 往返字节相等、小响应不压、`identity` 不压。
- `src/incremental.ts` + `scripts/chart-incremental.test.mjs`：图表尾部增量判定的纯函数与测试（追加、最后一根改写、缩短/乱序回退、指标前缀兼容）。
- `STEP.d/06-frontend-incremental.md`：前端增量更新范围与指标重算成本。
