# UDF 重构、协议修复与回测

## 模块一 阶段

1. **R0 契约锁定**（已落地）：`tests/udf_contract_test.rs` 固化 `/udf/*` JSON 形状，防止拆分走样。
2. **R1 模块拆分**（已落地）：`src/app/udf.rs`（567 行单文件）→ `src/app/udf/`（子模块），行为不变。
3. **R2 文档**（已落地）：本文件 + `./STEP.d/*` + `./CHANGELOG.md` + README 同步。
4. **F1 pricescale**（已落地）：按真实成交价采样推导，静态表仅作下限。
5. **F2 history 语义**（已落地）：`countback` 优先于 `from`、`to` 开区间、`no_data` 带 `nextTime`。
6. **F3 周期与 session**（已落地）：`1W`/`1M`/`3D`、`has_weekly_and_monthly`、FX `session`。
7. **G 验收**：静态检查 + 全量测试 + 文档 + 冒烟。
8. **B0 架构裁决**（已落地）：Rust API/数据/图表层 + Python vectorbt worker，
   见 `./docs/adr/0001-rust-python-vectorbt.md`。
9. **B1 Python worker**（已落地）：Binance 现货 12 周期读取、双均线只做多、vectorbt、
   回撤事件、指标、参数扫描、CLI。
10. **B2 Rust 回测模块**（已落地）：配置、`002_backtest.sql`、任务状态机、子进程调用、
    API、对比与 CSV。
11. **B3 前端**（已落地）：回测面板、运行列表、详情、回撤/水下曲线、10 个对比。
12. **B4 验收**（已落地）：Python 35 项单测（含独立参考实现对账）、Rust 门禁、
    真实数据冒烟、四层验证（`./docs/validation.md`）、文档与变更记录。

## 模块二 验收命令

1. 在数据仓库所在机器执行以下命令，全部零错误。

```bash
cargo fmt -- --check
cargo clippy -p tradingview-backend --all-targets -- -D warnings
cargo test -p tradingview-backend
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items

cd python
TRADINGVIEW_REAL_DATA=/data/data /data/vbt/.venv/bin/python -m unittest discover -s tests -v

cd ../frontend
npm run build
npm test
```

真实数据 Python 用例需要装有 vectorbt 1.2.0 的解释器；没有真实数据时自动跳过。

## 模块三 约束

1. 旧 UDF 阶段只改 UDF 模块与文档；`/api/*` 契约、SQLite schema、`data_loader` /
   `routes` 对外行为不动。
2. 不新增 crate 依赖（旧 UDF 阶段）；回测阶段按需新增 `tokio` 的 `process`/`time`。
3. 每个阶段先写失败测试，再改实现。
4. 协议修复必须有官方语义依据，依据写进 `./STEP.d/02-udf-contract.md`。
5. 回测阶段只做 Binance 现货只做多；做空/永续/杠杆返回明确错误。
6. Python 与 Rust 的 JSON 契约必须双端测试，任一侧改字段需同步另一侧。

## 模块四 详细文档

- 模块拆分与不变量：`./STEP.d/01-udf-module.md`
- 协议修复清单（F1–F3）：`./STEP.d/02-udf-contract.md`
- 阶段二 缓存硬化（A1）：`./STEP.d/03-kline-cache.md`
- 阶段二 目录缓存（A2）：`./STEP.d/04-catalog-cache.md`
- 阶段二 响应压缩（A3）：`./STEP.d/05-compression.md`
- 阶段二 前端增量（A4）：`./STEP.d/06-frontend-incremental.md`
- 阶段三 数据质量报告：`./STEP.d/07-quality-report.md`
- 阶段四 回测/回撤/对比：`./STEP.d/08-backtest.md`
- 四层验证报告：`./docs/validation.md`
- 架构裁决：`./docs/adr/0001-rust-python-vectorbt.md`
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
4. 回测真实数据冒烟（2026-11-16，BTCUSDT 1h，54341 根）：
   - 单组 `fast=10/slow=30`：`succeeded`，1090 笔交易、67 个回撤事件、2000 点权益/水下曲线、2180 个买卖点标记；
   - 2×2 参数扫描：4 条运行共享同一 `batch_id`；
   - 100 组参数扫描（`fast=5..14` × `slow=20..29`）：100 条全部 `succeeded`，参数与运行一一对应，耗时 68.6 秒；从该批次勾选 10 个运行对比成功；
   - 2 个运行对比：2 条曲线、2010 个并集点、0 条元数据警告；CSV 3 行 × 29 列；
   - 11 个运行对比返回 400；`SIGTERM` 退出码 0；重启后原运行仍为 `succeeded`。
   - 冒烟同时暴露并修复：Binance 现货 2025-01-01 起归档时间戳为微秒，worker 现按数量级归一为毫秒。
5. 四层验证（2026-11-16）：正确性 35 项 Python 测试全绿，1m 全量单组中位
   13.38 秒、100 组 164.74 秒、5 并发 P95 22.9 毫秒，浏览器与安全项见
   `./docs/validation.md`。
