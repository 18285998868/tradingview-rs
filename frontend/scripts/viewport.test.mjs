/** 视窗加载与区间统计的纯函数测试。 */
import assert from 'node:assert/strict';
import test from 'node:test';

import { computeRangeStats, mergeKlines, timeframeMs, windowForViewport } from '../src/viewport.ts';

const MINUTE = 60_000;
const DAY = 24 * 60 * MINUTE;

function bar(time, open, high, low, close, volume) {
  return { time, open, high, low, close, volume };
}

test('合并分块按时间升序并去重', () => {
  const a = [bar(1, 10, 11, 9, 10.5, 100), bar(2, 10.5, 12, 10, 11, 200)];
  const b = [bar(2, 10.5, 12, 10, 11.5, 250), bar(3, 11.5, 13, 11, 12, 300)];
  const merged = mergeKlines([a, b]);
  assert.deepEqual(merged.map((k) => k.time), [1, 2, 3]);
  assert.equal(merged[1].volume, 250);
});

test('区间统计使用闭区间和首开末收', () => {
  const klines = [
    bar(0, 100, 105, 95, 102, 10),
    bar(MINUTE, 102, 108, 101, 107, 20),
    bar(2 * MINUTE, 107, 110, 106, 109, 30),
    bar(3 * MINUTE, 109, 112, 108, 111, 40),
  ];
  const stats = computeRangeStats(klines, MINUTE, 2 * MINUTE);
  assert.ok(stats);
  assert.equal(stats.bars, 2);
  assert.equal(stats.from, MINUTE);
  assert.equal(stats.to, 2 * MINUTE);
  assert.ok(Math.abs(stats.changePct - (109 / 102 - 1) * 100) < 1e-9);
  assert.equal(stats.high, 110);
  assert.equal(stats.low, 101);
  assert.equal(stats.volume, 50);
});

test('区间没有 bar 时返回 null', () => {
  const klines = [bar(0, 100, 105, 95, 102, 10)];
  assert.equal(computeRangeStats(klines, 5 * MINUTE, 6 * MINUTE), null);
  assert.equal(computeRangeStats([], 0, 1), null);
});

test('反向端点等价于正序', () => {
  const klines = [bar(0, 100, 105, 95, 102, 10), bar(MINUTE, 102, 108, 101, 107, 20)];
  const forward = computeRangeStats(klines, 0, MINUTE);
  const reverse = computeRangeStats(klines, MINUTE, 0);
  assert.deepEqual(reverse, forward);
});

test('周期步长覆盖 15 个标准周期', () => {
  assert.equal(timeframeMs('1m'), MINUTE);
  assert.equal(timeframeMs('1h'), 60 * MINUTE);
  assert.equal(timeframeMs('1d'), 24 * 60 * MINUTE);
  assert.equal(timeframeMs('1mo'), 30 * 24 * 60 * MINUTE);
  assert.equal(timeframeMs('7x'), null);
});

test('取数窗口两侧各加 500 根缓冲', () => {
  const viewport = { from: 1_000 * MINUTE, to: 1_500 * MINUTE };
  const window = windowForViewport(viewport, MINUTE, 500, 100_000);
  assert.equal(window.from, 500 * MINUTE);
  assert.equal(window.to, 2_000 * MINUTE);
});

test('超过上限时保留视窗并按中心截断', () => {
  const viewport = { from: 0, to: 500 * DAY };
  const window = windowForViewport(viewport, MINUTE, 500, 100_000);
  assert.equal(window.to - window.from, 100_000 * MINUTE);
  const center = (window.from + window.to) / 2;
  assert.equal(center, 250 * DAY);
});

test('视窗本身超过上限时按中心截断', () => {
  const viewport = { from: 0, to: 200_000 * MINUTE };
  const window = windowForViewport(viewport, MINUTE, 500, 100_000);
  assert.equal(window.to - window.from, 100_000 * MINUTE);
  assert.equal((window.from + window.to) / 2, 100_000 * MINUTE);
});

test('未知周期不加缓冲', () => {
  const viewport = { from: 1, to: 2 };
  assert.deepEqual(windowForViewport(viewport, 0, 500, 100_000), viewport);
});
