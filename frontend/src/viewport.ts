/** 视窗加载与区间统计的纯函数。 */

import type { Kline } from './types';

export interface Viewport {
  /** 毫秒时间戳（含）。 */
  from: number;
  /** 毫秒时间戳（含）。 */
  to: number;
}

export interface FetchWindow {
  from: number;
  to: number;
}

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
const WEEK = 7 * DAY;
const MONTH = 30 * DAY;

const TIMEFRAME_MS: Record<string, number> = {
  '1m': MINUTE,
  '3m': 3 * MINUTE,
  '5m': 5 * MINUTE,
  '15m': 15 * MINUTE,
  '30m': 30 * MINUTE,
  '1h': HOUR,
  '2h': 2 * HOUR,
  '4h': 4 * HOUR,
  '6h': 6 * HOUR,
  '8h': 8 * HOUR,
  '12h': 12 * HOUR,
  '1d': DAY,
  '3d': 3 * DAY,
  '1w': WEEK,
  '1mo': MONTH,
};

/** 周期对应的毫秒步长；未知周期返回 null。 */
export function timeframeMs(timeframe: string): number | null {
  return TIMEFRAME_MS[timeframe] ?? null;
}

/**
 * 计算同区间切换周期时的取数窗口：视窗两侧各留 `bufferBars` 根，
 * 总跨度不超过 `maxBars` 根。
 *
 * 超出上限时优先保留视窗本身；视窗本身都放不下才按中心截断。
 * `barMs <= 0` 表示周期未知，此时不加缓冲、不做上限收敛。
 */
export function windowForViewport(
  viewport: Viewport,
  barMs: number,
  bufferBars: number,
  maxBars: number,
): FetchWindow {
  if (!(barMs > 0) || !(maxBars > 0)) {
    return { from: viewport.from, to: viewport.to };
  }
  const pad = barMs * Math.max(0, bufferBars);
  const maxSpan = barMs * maxBars;
  const center = (viewport.from + viewport.to) / 2;
  const viewSpan = viewport.to - viewport.from;
  if (viewSpan >= maxSpan) {
    return { from: center - maxSpan / 2, to: center + maxSpan / 2 };
  }
  const maxPad = (maxSpan - viewSpan) / 2;
  const actualPad = Math.min(pad, maxPad);
  return { from: viewport.from - actualPad, to: viewport.to + actualPad };
}

export interface RangeStats {
  /** 区间内第一根 bar 的时间（毫秒）。 */
  from: number;
  /** 区间内最后一根 bar 的时间（毫秒）。 */
  to: number;
  /** 区间内的 bar 根数。 */
  bars: number;
  /** 首根开盘到末根收盘的涨跌幅（百分比）。 */
  changePct: number;
  /** 区间最高价。 */
  high: number;
  /** 区间最低价。 */
  low: number;
  /** 区间成交量之和。 */
  volume: number;
}

/**
 * 合并多个按时间升序的 K 线块。
 *
 * 同一时间戳后到的块覆盖先到的块；返回结果按时间升序。分块拉取时
 * 前后页在边界上可能重叠，靠时间去重比按索引拼接更稳。
 */
export function mergeKlines(chunks: Kline[][]): Kline[] {
  const byTime = new Map<number, Kline>();
  for (const chunk of chunks) {
    for (const kline of chunk) byTime.set(kline.time, kline);
  }
  return [...byTime.values()].sort((a, b) => a.time - b.time);
}

/** 计算闭区间 `[from, to]` 内的区间统计；没有 bar 时返回 null。 */
export function computeRangeStats(
  klines: Kline[],
  from: number,
  to: number,
): RangeStats | null {
  if (klines.length === 0) return null;
  const start = Math.min(from, to);
  const end = Math.max(from, to);
  let bars = 0;
  let first: Kline | null = null;
  let last: Kline | null = null;
  let high = -Infinity;
  let low = Infinity;
  let volume = 0;
  for (const kline of klines) {
    if (kline.time < start) continue;
    if (kline.time > end) break;
    if (first === null) first = kline;
    last = kline;
    bars += 1;
    if (kline.high > high) high = kline.high;
    if (kline.low < low) low = kline.low;
    volume += kline.volume;
  }
  if (first === null || last === null) return null;
  const changePct = first.open !== 0 ? (last.close / first.open - 1) * 100 : 0;
  return {
    from: first.time,
    to: last.time,
    bars,
    changePct,
    high,
    low,
    volume,
  };
}
