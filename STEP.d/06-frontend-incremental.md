# 阶段二 A4：前端图表增量更新

## 背景

回放每前进一根 bar 都会调用 `updateData(prefix)`，随后 `refreshIndicators()`：

1. `updateData` 对整段前缀重新 `toCandle` + `series.setData`；
2. `applyIndicators` 对每条指标线/柱重新 `setData` 全部点位；
3. `renderActiveIndicators` 每帧清空并重建指标条 DOM。

2000 根的窗口下，第 3 步是纯浪费；第 1、2 步则让 lightweight-charts 每帧重新吸收整段数据。分页加载（每次 200 根）也走同样的整段重绘。

## 方案

1. 新增 `src/incremental.ts`：
   - `isTailExtension(prev, next)`：`next` 是 `prev` 的尾部扩展（前缀逐根相同，只有最后一根允许改写 OHLCV，新 bar 时间严格递增）。
   - `tailValuesCompatible(prevValues, nextValues, prevLen)`：指标旧前缀逐点相同；上一根允许改写，但不能从“有值”变成“无值”（`series.update` 删不掉已写入的点）。
2. `chart.ts` 维护与 `allKlines` 对齐的 `candleData` 缓存：
   - `updateData` / `appendData` 满足尾部扩展时只对新增/改写的 bar 调 `series.update()`；
   - 缩短前缀（回放重置）、乱序、任意跳转仍然整段 `setData`；
   - `prependData` 受 lightweight-charts 限制仍整段 `setData`，但只转换新增的前缀，旧蜡烛直接复用缓存。
3. `applyIndicators` 记录上一次的指标 key、K 线与值：
   - key 相同、数据是尾部扩展、series/pane 形状不变、指标前缀值逐点相同时，只对尾部调 `series.update()`；
   - 任一条件不满足就走原来的整段 `setData` / 重建 series 路径；
   - 指标计算仍在完整前缀上执行（因果指标的前缀值不会因追加而变，但重算保证暖机与最新一根正确）。
4. `app.ts` 的 `renderActiveIndicators` 按指标 id 顺序做 memo，只在增删/换序时重建 DOM。

## 不变量

1. `/api/*` 契约、`ChartManager` 公开方法与调用方参数不变。
2. `updateData` 的视口跟随行为不变：回放每步仍把可见区间贴到最新 `visibleBars` 根。
3. 指标输入仍是“截止当前游标的前缀”，不会提前看到未来 bar；指标改参数、增删指标、换周期都会整段重算。
4. 任何无法证明“只有尾部变化”的情况都退回 `setData`，增量路径只是快路径，不是唯一路径。
5. `series.update()` 的时间必须不早于序列最后一个点；`isTailExtension` 已保证时间严格递增，指标路径额外保证“最后一根不从有值变无值”。

## 指标成本

2000 根 bar，Node 26 + technicalindicators 3.1，单次 `calculate` 取 10 次平均：

| 指标 | 耗时 |
|---|---|
| BB | 2.9 ms |
| StochRSI | 2.8 ms |
| CCI | 2.1 ms |
| TRIX | 2.0 ms |
| ADX | 1.8 ms |
| RSI + MACD + BB + SMA | 4.8 ms |
| 26 个指标全部 | 25.0 ms |

回放默认 1×（每秒 1 根），4×/8× 也只有每秒 4/8 次；常见指标组合 3–5 ms/帧，低于一帧预算。因此本轮不引入 Web Worker：Worker 需要把 K 线与指标结果在结构化克隆/Transferable 之间搬来搬去，收益不足以抵消复杂度。若以后把回放速度提到几十倍，或指标数量接近全选，再评估。

## 验证

正常安装依赖的机器：

```bash
cd frontend
npm test
```

远程 CachyOS 没有 npm，验证时用临时 `/data/node_modules`（typescript 5.9.3、lightweight-charts 5.2.1、technicalindicators 3.1.0、vite 6.4.2）：

```bash
cd frontend
node /data/node_modules/typescript/bin/tsc -p tsconfig.json --noEmit
node --test scripts/*.test.mjs
```

`tsc --noEmit` 通过，13 个前端测试全部通过。临时目录不进入仓库。

`npm test` 跑两个文件：

- `scripts/indicators.parity.test.mjs`：26 个指标的 parity（原有用例不变）；
- `scripts/chart-incremental.test.mjs`：尾部扩展、最后一根改写、缩短/改时间/乱序回退、指标前缀兼容、有值变无值回退。

## 浏览器冒烟

构建：Vite 6.4.2（临时 `node_modules` 放在仓库外，不进入仓库），104 个模块，产物 `index-*.js` 486.58 kB（gzip 148.74 kB）、`index-*.css` 6.09 kB（gzip 1.57 kB）。

运行：release 二进制 + `/tmp` 状态库 + headless Firefox（geckodriver 0.37.1），预置 `last_session` 为 `futures/BTCUSDT/1h`，指标 SMA + MACD。

| 操作 | 结果 |
|---|---|
| 首次加载 | 图表与 MACD 子窗格渲染，`400 条K线`（初始自动向右预取一页） |
| 进入回放 | `1801 / 2000`，光标在 2026-11-06 00:00 |
| 步进 5 次 | `1806 / 2000` |
| 播放 1.2 s 后暂停 | `1807 / 2000` |
| 重置 | 回到 `1801 / 2000`（前缀缩短，走整段 `setData` 回退） |
| 退出回放 | 恢复 `2000 条K线`，回放控件隐藏，视口回到进入前 |

全程指标条保持 SMA/MACD 两个 chip，未重建；控制台只有浏览器的 `ResizeObserver loop completed with undelivered notifications` 警告，没有应用异常。该冒烟覆盖了尾部增量、播放/步进、重置回退、退出全量重绘四条路径。

## 取舍

1. `prependData` 无法避免整段 `setData`：lightweight-charts 只提供 `setData` / `update`，没有 prepend。分页向左仍是 O(全量)，但只发生在用户滚动到左边缘时，频率远低于回放逐根前进。
2. 指标仍全量计算，只是全量“重绘”变成尾部更新。真正的增量指标计算需要为 26 个指标分别定义带状态的累加器，并与 parity 口径逐点对齐，风险和收益不匹配。
3. 尾部判定多了一次 O(n) 比较；相对 `setData` 的内部重建可以忽略，且判定失败即回退，不影响正确性。
4. 指标条 memo 只比较 id，不比较参数：参数变化只影响图形，不影响 chip 文案与事件绑定。
5. `frontend/dist` 是入库产物；改 `frontend/src` 后必须重建并提交 `dist`。本次已用 A4 源码重建，替换旧的 `index-5ndkP3nD.js`。
