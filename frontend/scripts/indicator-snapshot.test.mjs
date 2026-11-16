/**
 * AC-V05：前端指标与接口 JSON 快照 100% 一致。
 *
 * 口径：
 * - 输入固定为 `/api/klines` 的真实响应（`fixtures/klines-400.json`）；
 * - 26 个指标全部使用默认参数，逐 series 比对数值与 null 暖机位；
 * - 比对是精确相等（`deepStrictEqual`），不做容差；
 * - 数值正确性另由 `indicators.parity.test.mjs` 的独立参考实现覆盖。
 *
 * 更新快照前必须确认差异来自预期改动：node scripts/generate-indicator-snapshot.mjs
 */
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import { getAllIndicators } from '../src/indicators.ts';

const dir = new URL('./fixtures/', import.meta.url);
const klines = JSON.parse(readFileSync(new URL('klines-400.json', dir), 'utf8'));
const snapshot = JSON.parse(readFileSync(new URL('indicator-snapshot.json', dir), 'utf8'));

test('AC-V05 26 个指标与接口 JSON 快照逐点一致', () => {
  assert.equal(klines.length, 400);
  assert.equal(snapshot.source.bars, klines.length);
  assert.equal(snapshot.source.first_time, klines[0].time);
  assert.equal(snapshot.source.last_time, klines[klines.length - 1].time);

  const defs = getAllIndicators();
  assert.equal(defs.length, 26);
  assert.deepEqual(
    defs.map((def) => def.id),
    snapshot.indicators.map((item) => item.id),
  );

  let points = 0;
  for (const [index, def] of defs.entries()) {
    const expected = snapshot.indicators[index];
    const result = def.calculate(klines, { ...def.defaultParams });
    const series = [
      ...(result.overlay ?? []),
      ...(result.sub ?? []).flatMap((pane) => pane.series),
    ];
    assert.deepEqual(
      series.map((item) => ({ label: item.label, values: item.values })),
      expected.series,
      `${def.id}: 与快照不一致`,
    );
    for (const item of series) {
      assert.equal(item.values.length, klines.length, `${def.id}/${item.label}: 长度`);
      points += item.values.length;
    }
    assert.ok(series.length > 0, `${def.id}: 没有输出 series`);
  }
  assert.ok(points >= defs.length * klines.length, `覆盖点位过少: ${points}`);
});
