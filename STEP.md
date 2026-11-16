# UDF 重构与协议修复

## 模块一 阶段

1. **R0 契约锁定**（已落地）：`tests/udf_contract_test.rs` 固化 `/udf/*` JSON 形状，防止拆分走样。
2. **R1 模块拆分**（已落地）：`src/app/udf.rs`（567 行单文件）→ `src/app/udf/`（子模块），行为不变。
3. **R2 文档**（已落地）：本文件 + `./STEP.d/*` + `./CHANGELOG.md` + README 同步。
4. **F1 pricescale**（已落地）：按真实成交价采样推导，静态表仅作下限。
5. **F2 history 语义**（已落地）：`countback` 优先于 `from`、`to` 开区间、`no_data` 带 `nextTime`。
6. **F3 周期与 session**（已落地）：`1W`/`1M`/`3D`、`has_weekly_and_monthly`、FX `session`。
7. **G 验收**：静态检查 + 全量测试 + 文档 + 冒烟。

## 模块二 验收命令

1. 在数据仓库所在机器执行以下命令，全部零错误。

```bash
cargo fmt -- --check
cargo clippy -p tradingview-backend --all-targets -- -D warnings
cargo test -p tradingview-backend
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items
```

## 模块三 约束

1. 只改 UDF 模块与文档；`/api/*` 契约、SQLite schema、`data_loader` / `routes` 对外行为不动。
2. 不新增 crate 依赖。
3. 每个阶段先写失败测试，再改实现。
4. 协议修复必须有官方语义依据，依据写进 `./STEP.d/02-udf-contract.md`。

## 模块四 详细文档

- 模块拆分与不变量：`./STEP.d/01-udf-module.md`
- 协议修复清单（F1–F3）：`./STEP.d/02-udf-contract.md`
- 阶段二 缓存硬化（A1）：`./STEP.d/03-kline-cache.md`
- 阶段二 目录缓存（A2）：`./STEP.d/04-catalog-cache.md`
- 阶段二 响应压缩（A3）：`./STEP.d/05-compression.md`
- 阶段二 前端增量（A4）：`./STEP.d/06-frontend-incremental.md`
- 变更记录：`./CHANGELOG.md`

## 模块五 冒烟

1. 冒烟配置写到 `/tmp`，不碰仓库内被忽略的 `config/*.toml`。
2. 独立端口 3011 + `/tmp` 状态库，验证静态修复后的端点与优雅停机。

```bash
cat >/tmp/tradingview-smoke.toml <<'TOML'
host = "127.0.0.1"
port = 3011
data_dir = "/data/data"
db_path = "/tmp/tradingview-smoke.db"
TOML

TRADINGVIEW_CONFIG=/tmp/tradingview-smoke.toml ./target/debug/tradingview &
SRV_PID=$!
curl -fsS http://127.0.0.1:3011/api/health
curl -fsS "http://127.0.0.1:3011/udf/history?symbol=BINANCE:BTCUSDT&resolution=1W&countback=2"
kill -TERM "$SRV_PID"; wait "$SRV_PID"; echo "exit=$?"
```

3. 冒烟结果：全部端点通过，`SIGTERM` 退出码 0，二次启动可复用同一 SQLite 文件。
