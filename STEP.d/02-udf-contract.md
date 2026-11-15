# UDF 协议修复清单（F1–F3）

## 模块一 官方语义依据

1. `GET /history`：`to` 是**右开区间**；`countback` 优先于 `from`，设置后 `from` 应被忽略。
2. `no_data` 可带 `nextTime`（秒），表示**过去**最近的可用 bar。
3. `pricescale` 为 `10^n`（小数位），tick = `minmov / pricescale`。
4. resolution 格式：分钟 `x`、天 `xD`、周 `xW`、月 `xM`。
5. `session`：24/7 用 `24x7`；24/5 用 `0000-0000`（周一到周五，`Etc/UTC`）。
6. 周/月线需 `has_weekly_and_monthly: true`，并给出 `weekly_multipliers` / `monthly_multipliers`。

参考：

- UDF 端点与 `history` 语义：<https://www.tradingview.com/charting-library-docs/latest/connecting_data/UDF/>
- `LibrarySymbolInfo`（pricescale / session / multipliers）：<https://www.tradingview.com/charting-library-docs/latest/api/interfaces/Charting_Library.LibrarySymbolInfo>
- 交易时段格式：<https://www.tradingview.com/charting-library-docs/latest/connecting_data/time-and-sessions/Trading-Sessions>
- resolution 格式：<https://www.tradingview.com/charting-library-docs/latest/ui_elements/Resolution>

## 模块二 F1 pricescale

1. 现状：`pricescale` 按市场静态给出（futures/spot `100`，FX 按币种后缀）。
2. 目标：取该交易对**最粗可用周期**的最后若干根 bar，统计 OHLC 的最大小数位 `d`（上限 8），`pricescale = 10^max(静态下限, d)`。
3. 只提高精度、不降低：静态值作为下限，避免整数 fixture 把 tick 变粗。
4. 采样失败或无 bar 时直接回落到静态值，不影响 `/udf/symbols` 可用性。

```bash
cargo test -p tradingview-backend --test udf_contract_test pricescale
```

## 模块三 F2 history 语义

1. `countback`：一旦出现，忽略 `from`（也不校验 `from > to`），返回 `to` 之前最近 N 根。
2. `to`：改成右开，`before = to * 1000`（毫秒），不再 `+1`。
3. `from`：保持**含**，`after = from * 1000 - 1`。
4. `no_data`：窗口内为空时，用同一周期回查窗口下界之前最近一根，命中则返回 `{"s":"no_data","nextTime":<秒>}`。
5. `DataError::NotFound` 仍走纯 `no_data`，不做额外回查。

```bash
cargo test -p tradingview-backend --test udf_contract_test history
```

## 模块四 F3 周期与 session

1. futures 增加 `1W → 1w`、`1M → 1mo`、`3D → 3d`；dukascopy 仍只支持 `M1/H1/D1`。
2. `Resolution` 记录类别（intraday/day/week/month），`has_intraday` 不再把周/月算成日内。
3. `has_weekly_and_monthly` 按实际周/月数据给出；有数据时才输出 `weekly_multipliers` / `monthly_multipliers`。
4. FX `session` 改为 `0000-0000`；crypto 保持 `24x7`。
5. `/udf/config` 的 `supported_resolutions` 增加 `3D`、`1W`、`1M`。

```bash
cargo test -p tradingview-backend --test udf_contract_test
cargo test -p tradingview-backend --test udf_test
```
