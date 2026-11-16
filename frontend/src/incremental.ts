import type { Kline } from './types';

/** 两根 bar 的 OHLCV 是否完全一致（时间由调用方单独比较）。 */
export function sameBar(a: Kline, b: Kline): boolean {
  return (
    a.time === b.time &&
    a.open === b.open &&
    a.high === b.high &&
    a.low === b.low &&
    a.close === b.close &&
    a.volume === b.volume
  );
}

/**
 * `next` 是否是 `prev` 的尾部扩展：前缀逐根相同（只有最后一根允许改写
 * OHLCV），新追加的 bar 时间严格递增。
 *
 * 满足时图表可以用 `series.update()` 逐根写入；否则只能整段 `setData`。
 */
export function isTailExtension(prev: Kline[], next: Kline[]): boolean {
  if (prev.length === 0 || next.length < prev.length) return false;
  for (let i = 0; i < prev.length; i++) {
    const a = prev[i];
    const b = next[i];
    if (a.time !== b.time) return false;
    // 只有最后一根允许被实时改写；更早的 bar 变化必须整段重绘。
    if (i < prev.length - 1 && !sameBar(a, b)) return false;
  }
  for (let i = 1; i < next.length; i++) {
    if (next[i].time <= next[i - 1].time) return false;
  }
  return true;
}

/**
 * 指标值前缀是否允许尾部增量：旧前缀必须逐点相同；上一根允许改写，
 * 但不能从"有值"变成"无值"（`series.update` 删不掉已经写进去的点）。
 */
export function tailValuesCompatible(
  prevValues: (number | null)[],
  nextValues: (number | null)[],
  prevLen: number,
): boolean {
  for (let i = 0; i < prevLen - 1; i++) {
    if (prevValues[i] !== nextValues[i]) return false;
  }
  const last = prevLen - 1;
  if (last >= 0 && prevValues[last] !== null) {
    const after = nextValues[last];
    if (after === null || after === undefined) return false;
  }
  return true;
}
