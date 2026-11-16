//! 数据质量报告：时间缺口、零成交与 OHLC 异常统计。
//!
//! 统计口径基于已经过 reader 与本层 [`KlineColumns`] 清洗的列集：
//! reader 会拒绝重复时间戳、非正价格和非法 OHLC，本层再丢弃非有限值并
//! 去重。因此 `non_finite_rows` / `duplicate_rows` 是兜底清洗的计数，
//! `bad_ohlc_bars` / `non_positive_price_bars` 是对清洗后残留值的检查；
//! 若数据源未来放宽校验，这些计数会直接暴露出来。
//!
//! [`KlineColumns`]: super::data_loader::KlineColumns

use serde::Serialize;

use super::data_loader::{DataError, KlineColumns, MarketType};

/// 报告中最多列出的缺口条数，其余只计数。
pub const MAX_REPORTED_GAPS: usize = 20;

/// 一个缺口：两根相邻 bar 之间缺失的 bar。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QualityGap {
    /// 缺口前最后一根 bar 的开盘时间（毫秒时间戳）。
    pub after: i64,
    /// 缺口后第一根 bar 的开盘时间（毫秒时间戳）。
    pub before: i64,
    /// 缺失的 bar 数。
    pub missing_bars: usize,
}

/// 数据质量报告。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QualityReport {
    /// 市场类型。
    pub market_type: String,
    /// 交易对。
    pub pair: String,
    /// 周期。
    pub timeframe: String,
    /// 实际 bar 数。
    pub bar_count: usize,
    /// 第一根 bar 的开盘时间（毫秒时间戳）。
    pub first_time: Option<i64>,
    /// 最后一根 bar 的开盘时间（毫秒时间戳）。
    pub last_time: Option<i64>,
    /// 按周期步长推算的应有 bar 数（首尾之间，含端点）。
    pub expected_bars: usize,
    /// 缺失 bar 数（`expected_bars - bar_count`）。
    pub missing_bars: usize,
    /// 覆盖率（`bar_count / expected_bars`，保留 6 位小数）。
    pub coverage: f64,
    /// 缺口段数（连续缺失算一段）。
    pub gap_count: usize,
    /// 单段缺口的最大缺失 bar 数。
    pub longest_gap_bars: usize,
    /// 缺口明细，最多 [`MAX_REPORTED_GAPS`] 条。
    pub gaps: Vec<QualityGap>,
    /// 缺口明细是否因超过上限而截断。
    pub gaps_truncated: bool,
    /// 成交量为 0 的 bar 数。
    pub zero_volume_bars: usize,
    /// 成交量为负的 bar 数。
    pub negative_volume_bars: usize,
    /// OHLC 包络非法的 bar 数（`high < low`、`high < open/close`、`low > open/close`）。
    pub bad_ohlc_bars: usize,
    /// 任一价格非正的 bar 数。
    pub non_positive_price_bars: usize,
    /// 清洗阶段丢弃的重复时间戳行数。
    pub duplicate_rows: usize,
    /// 清洗阶段丢弃的非有限值行数。
    pub non_finite_rows: usize,
}

/// bar 步长：固定毫秒，或长度不固定的自然月。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BarStep {
    /// 固定毫秒步长。
    Fixed(i64),
    /// 自然月（binance `1mo`）。
    CalendarMonth,
}

impl BarStep {
    /// 按市场与周期推导步长。
    ///
    /// `load_all_klines` 已在读盘前校验过周期，这里重复解析只是为了让
    /// 缺口统计拿到该周期的真实步长；失败仍按 400 映射。
    pub(crate) fn for_market_timeframe(
        market_type: MarketType,
        timeframe: &str,
    ) -> Result<Self, DataError> {
        match market_type {
            MarketType::Dukascopy => {
                let timeframe = timeframe
                    .parse::<dukascopy_core::Timeframe>()
                    .map_err(|e| DataError::InvalidParam(e.to_string()))?;
                Ok(Self::Fixed(timeframe.interval_ms()))
            }
            _ => {
                let interval: binance_core::Interval = timeframe
                    .parse()
                    .map_err(|e: String| DataError::InvalidParam(e))?;
                Ok(match interval.step_ms() {
                    Some(ms) => Self::Fixed(ms),
                    None => Self::CalendarMonth,
                })
            }
        }
    }

    /// 两根已有 bar 之间缺了多少根。
    pub(crate) fn missing_between(self, after: i64, before: i64) -> usize {
        if before <= after {
            return 0;
        }
        match self {
            Self::Fixed(step) => {
                let diff = before - after;
                if diff <= step {
                    0
                } else {
                    usize::try_from((diff - 1) / step).unwrap_or(usize::MAX)
                }
            }
            Self::CalendarMonth => {
                let mut missing = 0usize;
                let mut cursor = after;
                while let Some(next) = binance_core::Interval::MO1.next_open(cursor) {
                    if next >= before || next <= cursor {
                        break;
                    }
                    missing += 1;
                    cursor = next;
                }
                missing
            }
        }
    }
}

/// 统计列集并生成报告。
pub(crate) fn analyze(
    market_type: MarketType,
    pair: &str,
    timeframe: &str,
    columns: &KlineColumns,
    step: BarStep,
) -> QualityReport {
    let mut zero_volume_bars = 0usize;
    let mut negative_volume_bars = 0usize;
    let mut bad_ohlc_bars = 0usize;
    let mut non_positive_price_bars = 0usize;

    for i in 0..columns.len() {
        let open = columns.open[i];
        let high = columns.high[i];
        let low = columns.low[i];
        let close = columns.close[i];
        let volume = columns.volume[i];

        if volume == 0.0 {
            zero_volume_bars += 1;
        } else if volume < 0.0 {
            negative_volume_bars += 1;
        }
        if open <= 0.0 || high <= 0.0 || low <= 0.0 || close <= 0.0 {
            non_positive_price_bars += 1;
        }
        if high < low || high < open || high < close || low > open || low > close {
            bad_ohlc_bars += 1;
        }
    }

    let bar_count = columns.len();
    let mut expected_bars = bar_count;
    let mut gap_count = 0usize;
    let mut longest_gap_bars = 0usize;
    let mut gaps = Vec::new();
    for i in 1..bar_count {
        let missing = step.missing_between(columns.time[i - 1], columns.time[i]);
        if missing == 0 {
            continue;
        }
        expected_bars = expected_bars.saturating_add(missing);
        gap_count += 1;
        longest_gap_bars = longest_gap_bars.max(missing);
        if gaps.len() < MAX_REPORTED_GAPS {
            gaps.push(QualityGap {
                after: columns.time[i - 1],
                before: columns.time[i],
                missing_bars: missing,
            });
        }
    }

    let missing_bars = expected_bars.saturating_sub(bar_count);
    let coverage = if expected_bars == 0 {
        0.0
    } else {
        round6(bar_count as f64 / expected_bars as f64)
    };
    let clean = columns.clean_stats();

    QualityReport {
        market_type: market_type.to_string(),
        pair: pair.to_owned(),
        timeframe: timeframe.to_owned(),
        bar_count,
        first_time: columns.time.first().copied(),
        last_time: columns.time.last().copied(),
        expected_bars,
        missing_bars,
        coverage,
        gap_count,
        longest_gap_bars,
        gaps_truncated: gap_count > gaps.len(),
        gaps,
        zero_volume_bars,
        negative_volume_bars,
        bad_ohlc_bars,
        non_positive_price_bars,
        duplicate_rows: clean.duplicate,
        non_finite_rows: clean.non_finite,
    }
}

/// 覆盖率保留 6 位小数，避免 JSON 里出现长尾浮点噪声。
fn round6(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE_MS: i64 = 1_704_067_200_000;
    const HOUR_MS: i64 = 3_600_000;

    #[test]
    fn fixed_step_counts_adjacent_and_missing_bars() {
        let step = BarStep::Fixed(HOUR_MS);
        assert_eq!(step.missing_between(BASE_MS, BASE_MS + HOUR_MS), 0);
        assert_eq!(step.missing_between(BASE_MS, BASE_MS + 4 * HOUR_MS), 3);
        // 小于一个步长的异常间隔不算缺 bar。
        assert_eq!(step.missing_between(BASE_MS, BASE_MS + HOUR_MS / 2), 0);
        // 非整倍步长按缺失 1 根计。
        assert_eq!(step.missing_between(BASE_MS, BASE_MS + 3 * HOUR_MS / 2), 1);
    }

    #[test]
    fn calendar_month_step_handles_variable_months() {
        // 2024-02-01 → 2024-04-01 中间缺 3 月（2 月有数据，3 月缺失）。
        let feb = 1_706_745_600_000;
        let apr = 1_711_929_600_000;
        assert_eq!(BarStep::CalendarMonth.missing_between(feb, apr), 1);
        // 1 月 → 4 月缺 2、3 月。
        assert_eq!(BarStep::CalendarMonth.missing_between(BASE_MS, apr), 2);
    }

    #[test]
    fn analyze_reports_gaps_and_anomalies() {
        let columns = KlineColumns::from_parts((
            vec![
                BASE_MS,
                BASE_MS + HOUR_MS,
                BASE_MS + 2 * HOUR_MS,
                BASE_MS + 3 * HOUR_MS,
                BASE_MS + 6 * HOUR_MS,
            ],
            vec![100.0, 102.0, 10.0, 0.0, 106.0],
            vec![105.0, 107.0, 9.0, 0.0, 109.0],
            vec![95.0, 97.0, 8.0, 0.0, 99.0],
            vec![102.0, 104.0, 10.5, 0.0, 108.0],
            vec![10.0, 0.0, 30.0, 0.0, 50.0],
        ));

        let report = analyze(
            MarketType::Futures,
            "BTCUSDT",
            "1h",
            &columns,
            BarStep::Fixed(HOUR_MS),
        );

        assert_eq!(report.bar_count, 5);
        assert_eq!(report.first_time, Some(BASE_MS));
        assert_eq!(report.last_time, Some(BASE_MS + 6 * HOUR_MS));
        assert_eq!(report.expected_bars, 7);
        assert_eq!(report.missing_bars, 2);
        assert_eq!(report.coverage, 0.714286);
        assert_eq!(report.gap_count, 1);
        assert_eq!(report.longest_gap_bars, 2);
        assert_eq!(
            report.gaps,
            vec![QualityGap {
                after: BASE_MS + 3 * HOUR_MS,
                before: BASE_MS + 6 * HOUR_MS,
                missing_bars: 2,
            }]
        );
        assert!(!report.gaps_truncated);
        assert_eq!(report.zero_volume_bars, 2);
        assert_eq!(report.negative_volume_bars, 0);
        assert_eq!(report.bad_ohlc_bars, 1);
        assert_eq!(report.non_positive_price_bars, 1);
        assert_eq!(report.duplicate_rows, 0);
        assert_eq!(report.non_finite_rows, 0);
    }

    #[test]
    fn analyze_carries_clean_counts_and_caps_gap_list() {
        // 前两根时间戳重复（丢弃第二根），随后一根 open=NaN（整行丢弃），
        // 其余每隔一根留一个缺口，用于验证缺口明细上限。
        let mut time = vec![BASE_MS, BASE_MS, BASE_MS + HOUR_MS];
        let mut open = vec![100.0, 100.0, f64::NAN];
        for i in 2..(MAX_REPORTED_GAPS + 5) {
            time.push(BASE_MS + (i as i64) * 2 * HOUR_MS);
            open.push(100.0);
        }
        let n = time.len();
        let columns = KlineColumns::from_parts((
            time,
            open,
            vec![101.0; n],
            vec![99.0; n],
            vec![100.0; n],
            vec![1.0; n],
        ));

        let report = analyze(
            MarketType::Futures,
            "BTCUSDT",
            "1h",
            &columns,
            BarStep::Fixed(HOUR_MS),
        );
        assert_eq!(report.duplicate_rows, 1);
        assert_eq!(report.non_finite_rows, 1);
        assert!(report.gap_count > MAX_REPORTED_GAPS);
        assert_eq!(report.gaps.len(), MAX_REPORTED_GAPS);
        assert!(report.gaps_truncated);
    }
}
