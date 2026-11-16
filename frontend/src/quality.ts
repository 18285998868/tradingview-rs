import type { QualityReport } from './types';

export type QualityLevel = 'good' | 'warn' | 'bad';

export interface QualityBadgeView {
  level: QualityLevel;
  text: string;
  title: string;
}

/** 覆盖率显示为百分比，最多保留两位小数并去掉多余的 0。 */
export function formatQualityPercent(coverage: number): string {
  if (!Number.isFinite(coverage)) return '—';
  const pct = Math.max(0, Math.min(1, coverage)) * 100;
  return `${pct.toFixed(2).replace(/\.?0+$/, '')}%`;
}

/** OHLC / 成交 / 清洗异常总数；任一非零都值得标红。 */
function anomalyCount(report: QualityReport): number {
  return (
    report.bad_ohlc_bars +
    report.non_positive_price_bars +
    report.negative_volume_bars +
    report.duplicate_rows +
    report.non_finite_rows
  );
}

/**
 * 把 `/api/quality` 报告压成信息栏徽标。
 *
 * 等级只由覆盖率与异常决定：零成交是业务信号（例如外汇周末），
 * 单独出现时仍保持绿色，避免把正常归档标成故障。
 */
export function qualityBadge(report: QualityReport): QualityBadgeView {
  const anomalies = anomalyCount(report);
  const level: QualityLevel =
    anomalies > 0 || report.coverage < 0.99
      ? 'bad'
      : report.gap_count > 0 || report.coverage < 0.999
        ? 'warn'
        : 'good';

  const parts = [`质量 ${formatQualityPercent(report.coverage)}`];
  if (report.missing_bars > 0) parts.push(`缺口 ${report.missing_bars}根`);
  if (report.zero_volume_bars > 0) parts.push(`零量 ${report.zero_volume_bars}`);
  if (anomalies > 0) parts.push(`异常 ${anomalies}`);

  const title = [
    `${report.pair} ${report.timeframe} · ${report.bar_count}/${report.expected_bars} 根 · 覆盖 ${formatQualityPercent(report.coverage)}`,
    `缺口 ${report.gap_count} 段 / ${report.missing_bars} 根${report.longest_gap_bars > 0 ? `，最长 ${report.longest_gap_bars} 根` : ''}${report.gaps_truncated ? '，明细已截断' : ''}`,
    `零成交 ${report.zero_volume_bars} · 负成交 ${report.negative_volume_bars}`,
    `OHLC 异常 ${report.bad_ohlc_bars} · 非正价格 ${report.non_positive_price_bars}`,
    `重复时间戳 ${report.duplicate_rows} · 非有限值 ${report.non_finite_rows}`,
  ].join('\n');

  return { level, text: parts.join(' · '), title };
}
