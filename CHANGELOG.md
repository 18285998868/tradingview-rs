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

### Added

- `tests/udf_contract_test.rs`：锁定 `/udf/config`、`/udf/symbols`、`/udf/search`、`/udf/history` 的 JSON 契约与官方语义（`to` 右开、`countback` 优先、`nextTime`、`pricescale` 采样、周线能力）。
- `STEP.md` 与 `STEP.d/`：拆分范围、协议修复清单与官方语义依据。
- 缓存测试：权重按字节换算、权重确实注册进缓存、8 线程并发未命中只读一次盘、错误分类往返。
- 目录缓存测试：扫描结果在 TTL 内复用、`invalidate_cache` 后立刻可见、按市场分键。
- `STEP.d/03-kline-cache.md`：缓存硬化的设计取舍与验证命令。
- `STEP.d/04-catalog-cache.md`：目录扫描缓存的设计与验证。
