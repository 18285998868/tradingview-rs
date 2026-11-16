/**
 * 数据质量徽标的纯函数测试。
 *
 * 运行：npm test（Node 直接 strip TS 类型后 import src/quality.ts）。
 */
import assert from 'node:assert/strict';
import test from 'node:test';

import { formatQualityPercent, qualityBadge } from '../src/quality.ts';

function report(overrides = {}) {
  return {
    market_type: 'futures',
    pair: 'BTCUSDT',
    timeframe: '1h',
    bar_count: 100,
    first_time: 0,
    last_time: 99 * 3_600_000,
    expected_bars: 100,
    missing_bars: 0,
    coverage: 1,
    gap_count: 0,
    longest_gap_bars: 0,
    gaps: [],
    gaps_truncated: false,
    zero_volume_bars: 0,
    negative_volume_bars: 0,
    bad_ohlc_bars: 0,
    non_positive_price_bars: 0,
    duplicate_rows: 0,
    non_finite_rows: 0,
    ...overrides,
  };
}

test('覆盖率为 1 且无异常时显示绿色质量徽标', () => {
  const view = qualityBadge(report());
  assert.equal(view.level, 'good');
  assert.equal(view.text, '质量 100%');
  assert.match(view.title, /BTCUSDT 1h · 100\/100 根/);
  assert.match(view.title, /缺口 0 段 \/ 0 根/);
});

test('零成交是业务信号，不单独标红', () => {
  const view = qualityBadge(report({ zero_volume_bars: 237 }));
  assert.equal(view.level, 'good');
  assert.equal(view.text, '质量 100% · 零量 237');
});

test('少量缺口显示黄色并标出缺失根数', () => {
  const view = qualityBadge(
    report({
      bar_count: 1000,
      expected_bars: 1001,
      missing_bars: 1,
      coverage: 0.999001,
      gap_count: 1,
      longest_gap_bars: 1,
      gaps: [{ after: 0, before: 2 * 3_600_000, missing_bars: 1 }],
    }),
  );
  assert.equal(view.level, 'warn');
  assert.equal(view.text, '质量 99.9% · 缺口 1根');
});

test('OHLC 异常与清洗异常标红', () => {
  const ohlc = qualityBadge(report({ bad_ohlc_bars: 1 }));
  assert.equal(ohlc.level, 'bad');
  assert.match(ohlc.text, /异常 1/);

  const duplicate = qualityBadge(report({ duplicate_rows: 2 }));
  assert.equal(duplicate.level, 'bad');
  assert.match(duplicate.text, /异常 2/);
});

test('覆盖率低于 99% 标红', () => {
  const view = qualityBadge(
    report({ expected_bars: 200, missing_bars: 100, coverage: 0.5, gap_count: 1 }),
  );
  assert.equal(view.level, 'bad');
  assert.equal(view.text, '质量 50% · 缺口 100根');
});

test('百分比最多保留两位小数并去掉尾零', () => {
  assert.equal(formatQualityPercent(1), '100%');
  assert.equal(formatQualityPercent(0.9987), '99.87%');
  assert.equal(formatQualityPercent(0.995), '99.5%');
  assert.equal(formatQualityPercent(0), '0%');
  assert.equal(formatQualityPercent(Number.NaN), '—');
});
