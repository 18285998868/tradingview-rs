# UDF 模块拆分（R1）

## 模块一 文件职责

1. `src/app/udf/mod.rs`：模块文档与 handler 重导出。
2. `src/app/udf/handlers.rs`：axum handler、查询参数、`spawn_blocking` 边界、错误映射。
3. `src/app/udf/symbol.rs`：`ParsedSymbol`、`SymbolError`、`parse_symbol`、交易所与品种元数据。
4. `src/app/udf/resolution.rs`：`Resolution`、`ResolvedResolution`、`ResolutionError`、本地周期 ↔ UDF resolution。
5. `src/app/udf/history.rs`：`HistoryWindow`、`HistoryError`、`MAX_UDF_BARS`。
6. `src/app/udf/price.rs`：按样本价格推导 `pricescale`（静态值作下限）。
7. `src/app/udf/protocol.rs`：`ConfigResponse` / `SymbolInfo` / `SearchItem` / `HistoryResponse` / `UdfError`。

## 模块二 不变量

1. 响应字段名、字段顺序、缺省值与拆分前逐字节一致（`serde` 结构体按声明顺序序列化）。
2. handler 仍是 `spawn_blocking` 包裹读盘与目录扫描，不阻塞 worker。
3. 错误分类不变：`DataError::NotFound` → `no_data`；`InvalidParam` → 原样 `errmsg`；其余 → `internal error` 且 `tracing::error!`。
4. `1M` / `1W` 在 R1 阶段仍被当作月线/周线拒绝，不会退化成 1 分钟；F3 才显式放开（见 `./02-udf-contract.md`）。
5. `MAX_UDF_BARS`、`DEFAULT_SEARCH_LIMIT`、`MAX_SEARCH_LIMIT` 数值不变。

## 模块三 验证

1. 契约测试必须全绿；协议行为改动必须同步修改断言，而不是绕开。

```bash
cargo test -p tradingview-backend --test udf_contract_test
```

2. 既有 UDF 集成测试必须全绿。

```bash
cargo test -p tradingview-backend --test udf_test
```

3. rustdoc 私有项检查必须零告警（拆分前 `src/config.rs` 有私有链接告警）。

```bash
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items
```
