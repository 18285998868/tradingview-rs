/** 运行记录排序：默认保持接口顺序，指标缺失的运行排在最后。 */
import type { BacktestRunSummary } from './types.ts';

/** 运行列表排序键。 */
export type RunSortKey = 'created' | 'return' | 'sharpe' | 'drawdown';

function metricValue(run: BacktestRunSummary, key: RunSortKey): number | null {
  const metrics = run.metrics;
  if (!metrics) return null;
  switch (key) {
    case 'return':
      return metrics.total_return_pct;
    case 'sharpe':
      return metrics.sharpe_ratio;
    case 'drawdown':
      return metrics.max_drawdown_pct;
    default:
      return null;
  }
}

/** 按指定指标排序，返回新数组；缺失指标的运行始终排在最后。 */
export function sortRuns(
  runs: readonly BacktestRunSummary[],
  key: RunSortKey,
): BacktestRunSummary[] {
  const copy = [...runs];
  if (key === 'created') return copy;
  // 收益与夏普越大越好；最大回撤按深度正数升序，越小越好。
  const direction = key === 'drawdown' ? 1 : -1;
  copy.sort((left, right) => {
    const a = metricValue(left, key);
    const b = metricValue(right, key);
    if (a === null && b === null) return 0;
    if (a === null) return 1;
    if (b === null) return -1;
    if (a === b) return 0;
    return (a - b) * direction;
  });
  return copy;
}
