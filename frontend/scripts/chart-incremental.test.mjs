/**
 * 图表增量更新的纯函数测试。
 *
 * 运行：npm test（Node 直接 strip TS 类型后 import src/incremental.ts）。
 */
import assert from 'node:assert/strict';
import test from 'node:test';

import { isTailExtension, tailValuesCompatible } from '../src/incremental.ts';

const HOUR = 3_600_000;

function bar(time, close = 100) {
  return { time, open: close - 1, high: close + 1, low: close - 2, close, volume: 10 };
}

function bars(n, start = 0) {
  return Array.from({ length: n }, (_, i) => bar(start + i * HOUR, 100 + i));
}

test('追加新 bar 是尾部扩展', () => {
  const prev = bars(3);
  assert.equal(isTailExtension(prev, [...prev, bar(3 * HOUR, 103)]), true);
});

test('相同数据是尾部扩展', () => {
  assert.equal(isTailExtension(bars(3), bars(3)), true);
});

test('只有最后一根允许改写 OHLCV', () => {
  const prev = bars(3);
  const next = bars(3);
  next[2] = bar(2 * HOUR, 999);
  assert.equal(isTailExtension(prev, next), true);

  next[1] = bar(HOUR, 999);
  assert.equal(isTailExtension(prev, next), false);
});

test('缩短 / 最后一根改时间 / 追加乱序都退回全量', () => {
  const prev = bars(3);
  assert.equal(isTailExtension(prev, bars(2)), false);

  const moved = bars(3);
  moved[2] = bar(10 * HOUR, 103);
  assert.equal(isTailExtension(prev, moved), false);

  const unordered = [...prev, bar(HOUR, 103)];
  assert.equal(isTailExtension(prev, unordered), false);

  assert.equal(isTailExtension([], bars(3)), false);
});

test('指标前缀逐点相同才允许尾部 update', () => {
  const prev = [1, 2, 3, null, 5];
  assert.equal(tailValuesCompatible(prev, [1, 2, 3, null, 6], 5), true);
  assert.equal(tailValuesCompatible(prev, [1, 2, 99, null, 5], 5), false);
  assert.equal(tailValuesCompatible(prev, [1, 2, 3, null, null], 5), false);
});

test('上一根从无值变有值可以 update，从有值变无值必须全量', () => {
  assert.equal(tailValuesCompatible([1, null], [1, 2], 2), true);
  assert.equal(tailValuesCompatible([1, 2], [1, null], 2), false);
  assert.equal(tailValuesCompatible([], [1], 0), true);
});
