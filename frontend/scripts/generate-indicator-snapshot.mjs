/**
 * 生成 AC-V05 指标快照。
 *
 * 输入是 `/api/klines` 的真实 JSON（此处固定为 `fixtures/klines-400.json`），
 * 输出是全部 26 个指标在默认参数下的逐点数值。快照与输入一起入库，
 * `indicator-snapshot.test.mjs` 用它们做 100% 逐点回归。
 *
 * 运行：node scripts/generate-indicator-snapshot.mjs
 */
import { readFileSync, writeFileSync } from 'node:fs';

import { getAllIndicators } from '../src/indicators.ts';

const dir = new URL('./fixtures/', import.meta.url);
const klines = JSON.parse(readFileSync(new URL('klines-400.json', dir), 'utf8'));
const indicators = getAllIndicators().map((def) => {
  const result = def.calculate(klines, { ...def.defaultParams });
  const series = [
    ...(result.overlay ?? []),
    ...(result.sub ?? []).flatMap((pane) => pane.series),
  ];
  return {
    id: def.id,
    name: def.name,
    series: series.map((item) => ({ label: item.label, values: item.values })),
  };
});

const snapshot = {
  source: {
    endpoint: '/api/klines',
    market_type: 'futures',
    pair: 'BTCUSDT',
    timeframe: '1h',
    bars: klines.length,
    first_time: klines[0].time,
    last_time: klines[klines.length - 1].time,
  },
  indicators,
};

writeFileSync(
  new URL('indicator-snapshot.json', dir),
  `${JSON.stringify(snapshot, null, 1)}\n`,
);
const points = indicators.reduce(
  (sum, item) => sum + item.series.reduce((n, series) => n + series.values.length, 0),
  0,
);
console.log(`indicators=${indicators.length} points=${points} bars=${klines.length}`);
