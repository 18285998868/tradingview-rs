# 阶段二 A3：gzip 响应压缩

## 背景

`/api/klines` 全量 1m（43200 根）响应约 4.3 MB，1h 全量约 6.2 MB。JSON 里全是重复字段名与浮点数，压缩比天然很高；本地回环虽然带宽不是瓶颈，但前端首次加载、多标签页、以及将来从局域网访问时，传输量仍然值得压。

## 方案

1. `tower-http` 开启 `compression-gzip` feature，在 `app::run` 里挂 `CompressionLayer`。
2. 位置必须在 `fallback_service` 之后，作为最外层：axum 的 `Router::layer` 只作用于调用时已存在的路由/fallback，放在 `ServeDir` 之前就压不到前端静态资源。
3. 用 `CompressionLevel::Fastest`（gzip level 1），不用默认级别。
4. 依赖 tower-http 的默认谓词：只压 >32 字节、且不是图片 / SSE / gRPC 的响应，并自动补 `Vary: accept-encoding`。

## 数值取舍

release 构建，同一进程、缓存已预热，`/api/klines?pair=BTCUSDT&timeframe=1m`（43200 根）：

| 级别 | 传输大小 | 比率 | 耗时（plain → gzip） |
|---|---|---|---|
| 默认 | 848 KB | 19.7% | 34 ms → 86 ms |
| Fastest | 1.30 MB | 30.2% | 34 ms → 40 ms |

默认级别为了省 450 KB 传输多花约 45 ms CPU；对本地优先的服务（前端与 API 同机）不划算，所以选 Fastest。将来如果服务改成远程访问，可以只改这一行。

其他响应：`/udf/history` 1m×10000 根 506 KB → 152 KB（30%）；`/api/pairs` 11 KB → 3.4 KB（30%）。

## ETag 暂不实现

1. 当前唯一消费者是自带前端与本机 UDF，两者都直接 `fetch`，不做 HTTP 缓存协商；只加 `ETag` 不加 `Cache-Control`，浏览器不会发 `If-None-Match`。
2. 加 `Cache-Control: no-cache` 会让浏览器把 4 MB 级 JSON 落盘，并且每次回源校验；换来的是省一次 1.3 MB 传输，收益与磁盘/复杂度不成比例。
3. 等 A4 把前端改成增量加载、单次请求变小之后再评估：那时 304 的命中率更高，也可以只对窗口请求设 ETag。

## 不变量

1. 压缩层在 `fallback_service` 之后；否则静态资源不压。
2. 响应内容不变：`gunzip` 后必须与不带 `Accept-Encoding` 的响应字节相同（`tests/http_compression_test.rs` 锁定）。
3. `<32` 字节的响应不压（`/udf/time` 这类），避免压缩后反而更大。
4. 客户端显式 `Accept-Encoding: identity` 时不压。

## 验证

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items
```

`tests/http_compression_test.rs` 三个用例：gzip 往返字节相等且更小、小响应不压、`identity` 不压。

真实二进制冒烟：`curl -H 'Accept-Encoding: gzip' -D -` 确认返回 `content-encoding: gzip` 与 `vary: accept-encoding`，`curl --compressed` 解析出的 JSON 与明文一致。
