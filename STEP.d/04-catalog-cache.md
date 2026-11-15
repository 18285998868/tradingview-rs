# 阶段二 A2：目录扫描缓存

## 背景

`list_pairs` 要为每个品种探测数据文件（binance 看周期子目录里有没有 zip，dukascopy 递归找 `.bi5`），`list_timeframes` 要逐个周期目录核对文件。真实数据目录有几百个品种，一次 `/api/pairs` 就是数千次 `read_dir`。

而调用它的端点全是高频且响应很小的：`/api/pairs`、`/api/timeframes`、`/udf/search`、`/udf/symbols`（后者每次都要先拿周期列表才能回 `supported_resolutions`）。目录内容只在下载器落盘时变化，没有理由每个请求重扫。

## 方案

1. 两级缓存：`pairs_cache: Cache<MarketType, Arc<Vec<String>>>`、`timeframes_cache: Cache<(MarketType, String), Arc<Vec<String>>>`，容量分别是 8 条与 1024 条（按条数计，与 K 线缓存的字节计权互不影响）。
2. 短 TTL：`CATALOG_TIME_TO_LIVE = 60s`。目录会被外部下载器改动，60 秒是"新落盘数据最迟可见"的上界；比 K 线缓存的 30 分钟短得多，因为目录变化不需要解压就能确认。
3. 统一入口：新增 `cached_scan(cache, key, scan)`，内部仍是 `try_get_with`，即失败不缓存、同 key single-flight；`list_pairs` / `list_timeframes` 只做参数准备和 `Arc<Vec<_>>` 解引用拷贝，扫描逻辑分别移到 `scan_pairs` / `scan_timeframes`。
4. 失效：`invalidate_cache` 同时清空列集、交易对、周期三级缓存，测试与手工排障共用。

## 不变量

1. 缓存存 `Arc<Vec<String>>`，对外仍返回 `Vec<String>`（clone），不改变现有函数签名与 `/api/*` 契约。
2. 扫描错误经 `LoadError` 分发后仍是 500，且不会被缓存。
3. 键必须含市场：`Spot` 没有数据源，返回空列表，不能和 `Futures` 共用结果。
4. 目录新增/删除在 TTL 内不可见是有意行为；测试与运维通过 `invalidate_cache` 立即刷新。

## 验证

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

新增两个测试：`test_list_pairs_cached_until_invalidated`、`test_list_timeframes_cached_until_invalidated`，断言 TTL 内复用旧结果、失效后看到新文件、市场分键。

另修正 `test_dukascopy_lists_only_timeframes_with_files`：补 `.bi5` 后先 `invalidate_cache` 再断言，避免与 60 秒 TTL 的语义冲突。

## 取舍

1. 60 秒内看不到新下载的品种。下载器落盘后重启服务或调用 `invalidate_cache` 可立即刷新；若以后需要外部触发，可加一个受保护的管理端点（不在本次范围）。
2. 容量是常量，`Spot` 这类无数据源的市场也会占一个键，可忽略。
3. 目录缓存与 K 线缓存互不感知：新落盘的品种可能先能被 `/api/klines` 读到，最多 60 秒后才出现在 `/api/pairs`；已存在的列集则最多 30 分钟内不包含新追加的 bar。两个 TTL 都只限制陈旧度，不影响已有数据。
