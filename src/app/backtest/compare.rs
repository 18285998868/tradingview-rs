//! 回测对比：元数据警告、时间并集对齐与 CSV 导出。
//!
//! 对齐规则见 SPEC AC-C03：同区间按时间戳对齐，不同区间取时间并集，缺失段
//! 显式写 `null`，不做重采样、不静默填充。警告规则见 AC-C04：策略版本、数据
//! 区间、成本配置、标的或周期不一致时警告；参数不同不警告但必须随结果展示。

use std::collections::BTreeMap;

use super::model::{
    AlignedPoint, BacktestMetrics, BacktestRunSummary, CompareResponse, CompareSeries,
    CompareWarning, UnderwaterPoint,
};

/// 元数据警告类别。
pub(crate) const WARNING_MARKET: &str = "market";
/// 周期不一致。
pub(crate) const WARNING_TIMEFRAME: &str = "timeframe";
/// 策略实现版本不一致。
pub(crate) const WARNING_STRATEGY_VERSION: &str = "strategy_version";
/// 数据区间不一致。
pub(crate) const WARNING_DATA_RANGE: &str = "data_range";
/// 手续费/滑点不一致。
pub(crate) const WARNING_COST: &str = "cost";

/// 构建对比响应。`curves` 必须与 `runs` 等长且顺序一致。
pub(crate) fn build_compare(
    runs: Vec<BacktestRunSummary>,
    curves: Vec<Vec<UnderwaterPoint>>,
    max_compare_runs: usize,
) -> CompareResponse {
    let warnings = build_warnings(&runs);
    let union = union_times(&curves);
    let mut series = Vec::with_capacity(runs.len());
    for (run, curve) in runs.iter().zip(curves) {
        let by_time = curve
            .into_iter()
            .map(|point| (point.time, point.drawdown_pct))
            .collect::<BTreeMap<_, _>>();
        let mut missing_points = 0usize;
        let points = union
            .iter()
            .map(|&time| {
                let value = by_time.get(&time).copied();
                if value.is_none() {
                    missing_points += 1;
                }
                AlignedPoint {
                    time,
                    drawdown_pct: value,
                }
            })
            .collect::<Vec<_>>();
        series.push(CompareSeries {
            run_id: run.id.clone(),
            label: label_of(run),
            params_text: run.params_text.clone(),
            points,
            missing_points,
        });
    }
    CompareResponse {
        runs,
        warnings,
        series,
        union_points: union.len(),
        max_compare_runs,
    }
}

/// 生成元数据警告。参数（`fast`/`slow`）差异刻意不产生警告。
pub(crate) fn build_warnings(runs: &[BacktestRunSummary]) -> Vec<CompareWarning> {
    let mut warnings = Vec::new();
    push_group_warning(
        &mut warnings,
        runs,
        WARNING_MARKET,
        "标的不一致：对比结果来自不同市场或交易对",
        |run| format!("{}:{}", run.market_type, run.pair),
    );
    push_group_warning(
        &mut warnings,
        runs,
        WARNING_TIMEFRAME,
        "周期不一致：不同周期的回撤曲线按原始时间戳对齐，未重采样",
        |run| run.timeframe.clone(),
    );
    push_group_warning(
        &mut warnings,
        runs,
        WARNING_STRATEGY_VERSION,
        "策略版本不一致：结果可能来自不同的信号/成交口径",
        |run| format!("{}@{}", run.strategy, run.strategy_version),
    );
    push_group_warning(
        &mut warnings,
        runs,
        WARNING_DATA_RANGE,
        "数据区间不一致：请求区间或实际数据覆盖不同",
        |run| {
            format!(
                "{}..{}|{}..{}",
                option_text(run.start_ms),
                option_text(run.end_ms),
                option_text(run.data_first_ms),
                option_text(run.data_last_ms),
            )
        },
    );
    push_group_warning(
        &mut warnings,
        runs,
        WARNING_COST,
        "成本配置不一致：手续费或滑点不同",
        |run| format!("fee={},slippage={}", run.fee_bps, run.slippage_bps),
    );
    warnings
}

/// 把某个元数据维度取值不同的运行聚成一条警告。
fn push_group_warning(
    warnings: &mut Vec<CompareWarning>,
    runs: &[BacktestRunSummary],
    kind: &str,
    message: &str,
    value_of: impl Fn(&BacktestRunSummary) -> String,
) {
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for run in runs {
        groups
            .entry(value_of(run))
            .or_default()
            .push(run.id.clone());
    }
    if groups.len() <= 1 {
        return;
    }
    warnings.push(CompareWarning {
        kind: kind.to_owned(),
        message: message.to_owned(),
        run_ids: groups.into_values().flatten().collect(),
    });
}

fn option_text(value: Option<i64>) -> String {
    value.map_or_else(|| "-".to_owned(), |value| value.to_string())
}

/// 曲线时间戳的有序并集。
fn union_times(curves: &[Vec<UnderwaterPoint>]) -> Vec<i64> {
    let mut union = std::collections::BTreeSet::new();
    for curve in curves {
        for point in curve {
            union.insert(point.time);
        }
    }
    union.into_iter().collect()
}

/// 图例标签：参数 + 运行 ID 前缀，保证同参数重复运行时仍可区分。
fn label_of(run: &BacktestRunSummary) -> String {
    let short = run.id.get(..8).unwrap_or(&run.id);
    format!("{} [{}]", run.params_text, short)
}

/// 导出对比指标 CSV（一行一个运行）。
pub(crate) fn export_csv(runs: &[BacktestRunSummary]) -> String {
    const COLUMNS: [&str; 29] = [
        "run_id",
        "label",
        "pair",
        "timeframe",
        "strategy",
        "strategy_version",
        "params",
        "fee_bps",
        "slippage_bps",
        "initial_balance",
        "start_ms",
        "end_ms",
        "bars",
        "data_first_ms",
        "data_last_ms",
        "num_trades",
        "win_rate",
        "total_return_pct",
        "sharpe_ratio",
        "sortino_ratio",
        "max_drawdown_pct",
        "avg_drawdown_pct",
        "drawdown_count",
        "longest_drawdown_days",
        "longest_recovery_days",
        "drawdown_time_pct",
        "profit_factor",
        "expectancy",
        "final_equity",
    ];
    let mut out = String::new();
    out.push_str(&COLUMNS.join(","));
    out.push('\n');
    for run in runs {
        let metrics = run.metrics.as_ref();
        let fields = [
            csv_field(&run.id),
            csv_field(&label_of(run)),
            csv_field(&run.pair),
            csv_field(&run.timeframe),
            csv_field(&run.strategy),
            csv_field(&run.strategy_version),
            csv_field(&run.params_text),
            number(run.fee_bps),
            number(run.slippage_bps),
            number(run.initial_balance),
            option_number(run.start_ms.map(|value| value as f64)),
            option_number(run.end_ms.map(|value| value as f64)),
            option_number(run.bars.map(|value| value as f64)),
            option_number(run.data_first_ms.map(|value| value as f64)),
            option_number(run.data_last_ms.map(|value| value as f64)),
            metric_i64(metrics, |m| m.num_trades),
            metric(metrics, |m| m.win_rate),
            metric(metrics, |m| m.total_return_pct),
            metric(metrics, |m| m.sharpe_ratio),
            metric(metrics, |m| m.sortino_ratio),
            metric(metrics, |m| m.max_drawdown_pct),
            metric(metrics, |m| m.avg_drawdown_pct),
            metric_i64(metrics, |m| m.drawdown_count),
            metric(metrics, |m| m.longest_drawdown_days),
            metric(metrics, |m| m.longest_recovery_days),
            metric(metrics, |m| m.drawdown_time_pct),
            metric(metrics, |m| m.profit_factor),
            metric(metrics, |m| m.expectancy),
            metric(metrics, |m| m.final_equity),
        ];
        out.push_str(&fields.join(","));
        out.push('\n');
    }
    out
}

fn metric(
    metrics: Option<&BacktestMetrics>,
    field: impl Fn(&BacktestMetrics) -> Option<f64>,
) -> String {
    option_number(metrics.and_then(&field))
}

fn metric_i64(
    metrics: Option<&BacktestMetrics>,
    field: impl Fn(&BacktestMetrics) -> i64,
) -> String {
    metrics.map_or_else(String::new, |value| field(value).to_string())
}

fn number(value: f64) -> String {
    value.to_string()
}

fn option_number(value: Option<f64>) -> String {
    value.map_or_else(String::new, number)
}

/// RFC 4180 字段转义：含逗号、引号或换行时加引号并双写内部引号。
fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::backtest::model::{RunStatus, STRATEGY_DUAL_MA, STRATEGY_VERSION};

    fn run(id: &str, params: (u32, u32)) -> BacktestRunSummary {
        BacktestRunSummary {
            id: id.to_owned(),
            batch_id: "batch".into(),
            status: RunStatus::Succeeded,
            market_type: "spot".into(),
            pair: "BTCUSDT".into(),
            timeframe: "1h".into(),
            strategy: STRATEGY_DUAL_MA.into(),
            strategy_version: STRATEGY_VERSION.into(),
            params: super::super::model::DualMaParams {
                fast: params.0,
                slow: params.1,
            },
            params_text: format!("fast={},slow={}", params.0, params.1),
            fee_bps: 10.0,
            slippage_bps: 5.0,
            initial_balance: 10_000.0,
            start_ms: Some(1_000),
            end_ms: Some(2_000),
            created_at: 1,
            started_at: Some(2),
            finished_at: Some(3),
            error: None,
            log_path: None,
            bars: Some(10),
            data_first_ms: Some(1_000),
            data_last_ms: Some(1_900),
            engine: Some("vectorbt 1.2.0".into()),
            worker_version: Some("0.1.0".into()),
            metrics: Some(BacktestMetrics {
                num_trades: 2,
                win_rate: Some(50.0),
                total_return_pct: Some(1.5),
                sharpe_ratio: None,
                sortino_ratio: None,
                max_drawdown_pct: Some(3.0),
                avg_drawdown_pct: Some(1.5),
                drawdown_count: 1,
                longest_drawdown_days: Some(1.0),
                longest_recovery_days: Some(0.5),
                drawdown_time_pct: Some(20.0),
                profit_factor: Some(1.2),
                expectancy: Some(0.01),
                final_equity: Some(10_150.0),
                bars: 10,
                periods_per_year: 8_760.0,
            }),
            trades_truncated: false,
            retention: "downsampled_equity".into(),
        }
    }

    fn curve(points: &[(i64, f64)]) -> Vec<UnderwaterPoint> {
        points
            .iter()
            .map(|&(time, drawdown_pct)| UnderwaterPoint { time, drawdown_pct })
            .collect()
    }

    #[test]
    fn param_difference_does_not_warn() {
        let runs = vec![run("aaaaaaaa-1", (10, 30)), run("bbbbbbbb-2", (5, 20))];
        assert!(build_warnings(&runs).is_empty());
    }

    #[test]
    fn metadata_differences_warn_by_kind() {
        let mut other = run("bbbbbbbb-2", (5, 20));
        other.fee_bps = 20.0;
        other.timeframe = "4h".into();
        other.pair = "ETHUSDT".into();
        other.strategy_version = "0".into();
        other.start_ms = Some(500);
        other.data_first_ms = Some(500);
        let warnings = build_warnings(&[run("aaaaaaaa-1", (10, 30)), other]);
        let kinds = warnings
            .iter()
            .map(|warning| warning.kind.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert!(kinds.contains(WARNING_MARKET));
        assert!(kinds.contains(WARNING_TIMEFRAME));
        assert!(kinds.contains(WARNING_STRATEGY_VERSION));
        assert!(kinds.contains(WARNING_DATA_RANGE));
        assert!(kinds.contains(WARNING_COST));
        assert_eq!(warnings.len(), 5);
        for warning in &warnings {
            assert_eq!(warning.run_ids.len(), 2);
        }
    }

    #[test]
    fn alignment_uses_union_and_marks_missing() {
        let runs = vec![run("aaaaaaaa-1", (10, 30)), run("bbbbbbbb-2", (5, 20))];
        let curves = vec![
            curve(&[(100, 0.0), (200, -1.0), (300, -2.0)]),
            curve(&[(200, 0.0), (400, -3.0)]),
        ];
        let response = build_compare(runs, curves, 10);
        assert_eq!(response.union_points, 4);
        assert_eq!(
            response.series[0].points,
            vec![
                AlignedPoint {
                    time: 100,
                    drawdown_pct: Some(0.0)
                },
                AlignedPoint {
                    time: 200,
                    drawdown_pct: Some(-1.0)
                },
                AlignedPoint {
                    time: 300,
                    drawdown_pct: Some(-2.0)
                },
                AlignedPoint {
                    time: 400,
                    drawdown_pct: None
                },
            ]
        );
        assert_eq!(response.series[0].missing_points, 1);
        assert_eq!(response.series[1].missing_points, 2);
        assert_eq!(response.series[0].label, "fast=10,slow=30 [aaaaaaaa]");
    }

    #[test]
    fn csv_has_header_row_per_run_and_quotes_params() {
        let runs = vec![run("aaaaaaaa-1", (10, 30)), run("bbbbbbbb-2", (5, 20))];
        let csv = export_csv(&runs);
        let lines = csv.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("run_id,label,pair,timeframe,strategy,"));
        assert_eq!(lines[1].split('"').count() % 2, 1);
        assert!(lines[1].contains("\"fast=10,slow=30\""));
        assert!(lines[1].ends_with(",10150"));
        assert_eq!(lines[2].split(',').count(), lines[1].split(',').count());
    }

    #[test]
    fn csv_escapes_quotes_and_newlines() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("a\"b"), "\"a\"\"b\"");
        assert_eq!(csv_field("a\nb"), "\"a\nb\"");
    }
}
