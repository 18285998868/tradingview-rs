//! `pricescale` 推导：按真实报价小数位抬高精度。
//!
//! UDF 的 `pricescale` 是 `10^n`，tick = `minmov / pricescale`。数据源不提供
//! tick 元数据，只能从 OHLC 的实际小数位反推；反推只会**提高**精度，静态
//! 值始终作为下限，避免整数样本把 tick 变粗。

use super::super::data_loader::{DataLoader, Kline, MarketType};
use crate::utils::timeframe_order;

/// 采样 bar 数（最粗周期）。
const SAMPLE_BARS: usize = 200;
/// `pricescale` 上限（小数位上限 8）。
const MAX_PRICESCALE: u64 = 100_000_000;

/// 用最粗可用周期的最近若干根 bar 推导 `pricescale`。
///
/// 读盘失败或没有 bar 时返回 `fallback`，不影响 `/udf/symbols` 可用性。
pub(crate) fn infer(
    loader: &DataLoader,
    market: MarketType,
    pair: &str,
    timeframes: &[String],
    fallback: u64,
) -> u64 {
    let Some(coarsest) = timeframes
        .iter()
        .max_by_key(|timeframe| timeframe_order(timeframe))
    else {
        return fallback;
    };
    let klines = loader.load_klines_filtered(market, pair, coarsest, None, None, Some(SAMPLE_BARS));
    match klines {
        Ok(klines) => pricescale_from_klines(&klines, fallback),
        Err(_) => fallback,
    }
}

/// 采样价格的最大小数位换算成 `pricescale`，并以 `floor` 为下限。
fn pricescale_from_klines(klines: &[Kline], floor: u64) -> u64 {
    let decimals = klines
        .iter()
        .flat_map(|k| [k.open, k.high, k.low, k.close])
        .map(decimals)
        .max()
        .unwrap_or(0);
    let scale = 10_u64
        .checked_pow(decimals)
        .unwrap_or(MAX_PRICESCALE)
        .min(MAX_PRICESCALE);
    floor.max(scale)
}

/// 一个价格在十进制下的有效小数位（去尾零，上限 8）。
fn decimals(value: f64) -> u32 {
    if !value.is_finite() || value == 0.0 {
        return 0;
    }
    let text = value.to_string();
    let Some((_, fraction)) = text.split_once('.') else {
        return 0;
    };
    let fraction = fraction.trim_end_matches('0');
    u32::try_from(fraction.len()).unwrap_or(8).min(8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kline(open: f64, high: f64, low: f64, close: f64) -> Kline {
        Kline {
            time: 0,
            open,
            high,
            low,
            close,
            volume: 1.0,
        }
    }

    #[test]
    fn counts_significant_decimals() {
        assert_eq!(decimals(100.0), 0);
        assert_eq!(decimals(0.123_45), 5);
        assert_eq!(decimals(1.230_0), 2);
        assert_eq!(decimals(0.0), 0);
        assert_eq!(decimals(f64::NAN), 0);
    }

    #[test]
    fn raises_static_floor_but_never_lowers_it() {
        let integer = [kline(100.0, 105.0, 95.0, 102.0)];
        assert_eq!(pricescale_from_klines(&integer, 100), 100);

        let sub_cent = [kline(0.123_45, 0.123_47, 0.123_44, 0.123_46)];
        assert_eq!(pricescale_from_klines(&sub_cent, 100), 100_000);

        assert_eq!(pricescale_from_klines(&sub_cent, 1_000_000), 1_000_000);
    }
}
