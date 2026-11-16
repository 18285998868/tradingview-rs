/** 运行记录排序的纯函数测试。 */
import assert from 'node:assert/strict';
import test from 'node:test';

import { sortRuns } from '../src/backtest-sort.ts';

function run(id, { ret = null, sharpe = null, drawdown = null } = {}) {
  return {
    id,
    metrics:
      ret === null && sharpe === null && drawdown === null
        ? null
        : {
            num_trades: 1,
            win_rate: null,
            total_return_pct: ret,
            sharpe_ratio: sharpe,
            sortino_ratio: null,
            max_drawdown_pct: drawdown,
            avg_drawdown_pct: null,
            drawdown_count: 0,
            longest_drawdown_days: null,
            longest_recovery_days: null,
            drawdown_time_pct: null,
            profit_factor: null,
            expectancy: null,
            final_equity: null,
            bars: 10,
            periods_per_year: 8760,
          },
  };
}

const runs = [
  run('a', { ret: 5, sharpe: 1.2, drawdown: 8 }),
  run('b', { ret: 2, sharpe: 2.5, drawdown: 3 }),
  run('c', { ret: 1, sharpe: 0.4, drawdown: 12 }),
  run('d'),
];

test('默认排序保持接口顺序并返回新数组', () => {
  const sorted = sortRuns(runs, 'created');
  assert.deepEqual(sorted.map((r) => r.id), ['a', 'b', 'c', 'd']);
  assert.notEqual(sorted, runs);
  assert.deepEqual(runs.map((r) => r.id), ['a', 'b', 'c', 'd']);
});

test('收益降序，缺失指标排最后', () => {
  assert.deepEqual(sortRuns(runs, 'return').map((r) => r.id), ['a', 'b', 'c', 'd']);
});

test('夏普降序，缺失指标排最后', () => {
  assert.deepEqual(sortRuns(runs, 'sharpe').map((r) => r.id), ['b', 'a', 'c', 'd']);
});

test('最大回撤按深度升序，越小越好', () => {
  assert.deepEqual(sortRuns(runs, 'drawdown').map((r) => r.id), ['b', 'a', 'c', 'd']);
});

test('同值保持原有相对顺序，且不修改入参', () => {
  const tied = [run('x', { ret: 3 }), run('y', { ret: 3 }), run('z', { ret: 1 })];
  assert.deepEqual(sortRuns(tied, 'return').map((r) => r.id), ['x', 'y', 'z']);
  assert.deepEqual(tied.map((r) => r.id), ['x', 'y', 'z']);
});
