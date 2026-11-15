# 阶段二 A1：K 线缓存硬化

## 背景

旧策略是 moka `max_capacity(200)` 条 + `time_to_live(300s)`，有三个问题：

1. 条数不等于内存。单 bar 六列共 48 字节，1m 级长历史一条列集可达上百 MB；200 条在真实数据下没有上界，也无法回答"缓存最多占多少内存"。
2. TTL 到期即失效：热品种每 5 分钟被重读一次，冷品种却要等 TTL 才释放。
3. 并发未命中各自读盘。`spawn_blocking` 下 N 个首屏请求同时打同一个 zip，就是 N 次解压 + N 份内存峰值，最后只有一份进缓存。

## 方案

1. 按字节计权：`KlineColumns::weight_kib` 取六列 `Vec::capacity` 之和除以 1024（向上取整、至少 1），`max_capacity` 用同一单位设为 2 GiB。权重用容量而不是 `len`，避免漏算预留空间。
2. 双过期约束：`time_to_idle(10min)` 回收冷数据，`time_to_live(30min)` 给"外部下载器写入的新数据最迟多久可见"一个上界。只留 TTI 会让热 key 永远读不到新文件，只留 TTL 会周期性把热数据全部作废。
3. single-flight：`load_all_klines` 改用 `Cache::try_get_with`，同一 key 同一时刻只有一个线程执行 `read_all_klines`，其余线程阻塞等结果；读盘逻辑因此拆成独立方法，缓存写入交给 moka。
4. 错误分发：moka 以 `Arc<E>` 把同一个错误交给所有等待者，而 `DataError` 内含 `io::Error`，不是 `Clone`。新增内部 `LoadError`（`NotFound` / `InvalidParam` / `Other`）：前两类保留裸消息以维持 404/400，其余压成一条消息按 500 处理，`Display` 里仍带原始错误链。

## 不变量

1. `CACHE_CAPACITY_KIB` 与 `weight_kib` 必须同单位（KiB），改一个必须改另一个。
2. 权重按 `capacity` 计，不按 `len`。
3. 空列集权重至少 1 KiB，避免零权重条目挤不掉。
4. `DataError::NotFound` / `InvalidParam` 经过 single-flight 往返后分类不变（`tests::test_load_error_keeps_http_class` 锁定）。
5. 测试钩子（读盘计数、读盘延时）只在 `cfg(test)` 下编译进 `DataLoader`，生产结构体不增加字段。

## 验证

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items
```

结果：78 unit + 5 integration + 10 contract + 1 udf 全绿；`test_concurrent_misses_share_one_read` 用 8 线程 + 100ms 读盘延时断言只读一次盘。

真实数据冒烟（端口 3011、`/tmp` 状态库）：8 个并发 `/api/klines?pair=BTCUSDT&timeframe=5m` 全部 200 且载荷一致；`NOPEUSDT` 仍 404、`timeframe=zzz` 仍 400、`market_type=spot` 仍 404；SIGTERM 退出码 0，二次启动幂等。

## 待办

1. 容量与 TTL 目前是常量；需要按部署调整时再提升到 `Config`（属于配置面变更，单独评审）。
2. A2 已完成：见 `./04-catalog-cache.md`。
