//! UDF resolution 与本地周期的双向映射。

use super::super::data_loader::MarketType;

/// resolution 的粒度类别。
///
/// `has_intraday` 只能看日内，不能把周/月线算进去。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResolutionKind {
    /// 分钟/小时级。
    Intraday,
    /// 日线（含 `3D`）。
    Day,
    /// 周线。
    Week,
    /// 月线。
    Month,
}

/// 一个 UDF resolution。
///
/// `udf` 是对外的 canonical 字符串，`minutes` 只用于排序，`kind` 用于能力标记。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Resolution {
    udf: &'static str,
    minutes: u64,
    kind: ResolutionKind,
}

impl Resolution {
    const fn new(udf: &'static str, minutes: u64, kind: ResolutionKind) -> Self {
        Self { udf, minutes, kind }
    }

    /// UDF 字符串表示。
    pub(crate) fn as_udf(self) -> &'static str {
        self.udf
    }

    /// 是否为日线（`1D` / `3D`）。
    pub(crate) fn is_day(self) -> bool {
        self.kind == ResolutionKind::Day
    }

    /// 是否为日内周期。
    pub(crate) fn is_intraday(self) -> bool {
        self.kind == ResolutionKind::Intraday
    }

    /// 是否为周线。
    pub(crate) fn is_week(self) -> bool {
        self.kind == ResolutionKind::Week
    }

    /// 是否为月线。
    pub(crate) fn is_month(self) -> bool {
        self.kind == ResolutionKind::Month
    }

    /// 排序权重（分钟数）。
    pub(crate) fn order(self) -> u64 {
        self.minutes
    }

    /// 把 UDF resolution 映射到指定市场的本地周期。
    ///
    /// `1M` 是月线、`1m` 是 1 分钟：大小写必须先区分，再走大小写不敏感的分钟映射。
    pub(crate) fn parse(
        market: MarketType,
        raw: &str,
    ) -> Result<ResolvedResolution, ResolutionError> {
        let r = raw.trim();
        if r.eq_ignore_ascii_case("1w") || r.eq_ignore_ascii_case("w") {
            return match market {
                MarketType::Futures => Ok(ResolvedResolution {
                    resolution: Resolution::new("1W", 10_080, ResolutionKind::Week),
                    timeframe: "1w",
                }),
                _ => Err(ResolutionError::Unsupported(raw.to_string())),
            };
        }
        if r == "1M" || r == "M" || r.eq_ignore_ascii_case("1mo") {
            return match market {
                MarketType::Futures => Ok(ResolvedResolution {
                    resolution: Resolution::new("1M", 43_200, ResolutionKind::Month),
                    timeframe: "1mo",
                }),
                _ => Err(ResolutionError::Unsupported(raw.to_string())),
            };
        }
        let lower = r.to_ascii_lowercase();
        let (resolution, timeframe) = match market {
            MarketType::Futures => match lower.as_str() {
                "1" | "1m" => (Resolution::new("1", 1, ResolutionKind::Intraday), "1m"),
                "3" | "3m" => (Resolution::new("3", 3, ResolutionKind::Intraday), "3m"),
                "5" | "5m" => (Resolution::new("5", 5, ResolutionKind::Intraday), "5m"),
                "15" | "15m" => (Resolution::new("15", 15, ResolutionKind::Intraday), "15m"),
                "30" | "30m" => (Resolution::new("30", 30, ResolutionKind::Intraday), "30m"),
                "60" | "1h" => (Resolution::new("60", 60, ResolutionKind::Intraday), "1h"),
                "120" | "2h" => (Resolution::new("120", 120, ResolutionKind::Intraday), "2h"),
                "240" | "4h" => (Resolution::new("240", 240, ResolutionKind::Intraday), "4h"),
                "360" | "6h" => (Resolution::new("360", 360, ResolutionKind::Intraday), "6h"),
                "480" | "8h" => (Resolution::new("480", 480, ResolutionKind::Intraday), "8h"),
                "720" | "12h" => (Resolution::new("720", 720, ResolutionKind::Intraday), "12h"),
                "3d" => (Resolution::new("3D", 4_320, ResolutionKind::Day), "3d"),
                "1d" | "d" | "24h" => (Resolution::new("1D", 1_440, ResolutionKind::Day), "1d"),
                _ => return Err(ResolutionError::Unsupported(raw.to_string())),
            },
            MarketType::Dukascopy => match lower.as_str() {
                "1" | "1m" | "m1" => (Resolution::new("1", 1, ResolutionKind::Intraday), "M1"),
                "60" | "1h" | "h1" => (Resolution::new("60", 60, ResolutionKind::Intraday), "H1"),
                "1d" | "d" | "d1" | "24h" => {
                    (Resolution::new("1D", 1_440, ResolutionKind::Day), "D1")
                }
                _ => return Err(ResolutionError::Unsupported(raw.to_string())),
            },
            MarketType::Spot => return Err(ResolutionError::SpotUnavailable),
        };
        Ok(ResolvedResolution {
            resolution,
            timeframe,
        })
    }

    /// 本地周期 → UDF resolution；未知周期返回 `None`。
    pub(crate) fn from_timeframe(timeframe: &str) -> Option<Self> {
        Some(match timeframe.to_ascii_lowercase().as_str() {
            "1m" | "m1" => Self::new("1", 1, ResolutionKind::Intraday),
            "3m" => Self::new("3", 3, ResolutionKind::Intraday),
            "5m" => Self::new("5", 5, ResolutionKind::Intraday),
            "15m" => Self::new("15", 15, ResolutionKind::Intraday),
            "30m" => Self::new("30", 30, ResolutionKind::Intraday),
            "1h" | "h1" => Self::new("60", 60, ResolutionKind::Intraday),
            "2h" => Self::new("120", 120, ResolutionKind::Intraday),
            "4h" => Self::new("240", 240, ResolutionKind::Intraday),
            "6h" => Self::new("360", 360, ResolutionKind::Intraday),
            "8h" => Self::new("480", 480, ResolutionKind::Intraday),
            "12h" => Self::new("720", 720, ResolutionKind::Intraday),
            "1d" | "d1" => Self::new("1D", 1_440, ResolutionKind::Day),
            "3d" => Self::new("3D", 4_320, ResolutionKind::Day),
            "1w" => Self::new("1W", 10_080, ResolutionKind::Week),
            "1mo" => Self::new("1M", 43_200, ResolutionKind::Month),
            _ => return None,
        })
    }
}

/// resolution 解析结果：UDF 表示 + 该市场的本地周期。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResolvedResolution {
    pub(crate) resolution: Resolution,
    pub(crate) timeframe: &'static str,
}

/// resolution 解析失败。
#[derive(Debug, thiserror::Error)]
pub(crate) enum ResolutionError {
    /// 不支持的 resolution 字符串。
    #[error("unsupported resolution: {0}")]
    Unsupported(String),
    /// 本地没有 spot 数据源。
    #[error("spot is not available locally")]
    SpotUnavailable,
}

/// 把某交易对的本地周期转成 UDF resolution 列表（去重 + 排序）。
pub(crate) fn timeframes_to_resolutions(timeframes: &[String]) -> Vec<Resolution> {
    let mut out: Vec<Resolution> = timeframes
        .iter()
        .filter_map(|tf| Resolution::from_timeframe(tf))
        .collect();
    out.sort_by_key(|r| r.order());
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_udf_resolutions() {
        assert_eq!(
            Resolution::parse(MarketType::Futures, "60")
                .unwrap()
                .timeframe,
            "1h"
        );
        assert_eq!(
            Resolution::parse(MarketType::Futures, "1D")
                .unwrap()
                .timeframe,
            "1d"
        );
        assert_eq!(
            Resolution::parse(MarketType::Futures, "1W")
                .unwrap()
                .timeframe,
            "1w"
        );
        assert_eq!(
            Resolution::parse(MarketType::Futures, "1M")
                .unwrap()
                .timeframe,
            "1mo"
        );
        assert_eq!(
            Resolution::parse(MarketType::Futures, "3D")
                .unwrap()
                .timeframe,
            "3d"
        );
        assert_eq!(
            Resolution::parse(MarketType::Dukascopy, "1")
                .unwrap()
                .timeframe,
            "M1"
        );
        assert_eq!(
            Resolution::parse(MarketType::Dukascopy, "1D")
                .unwrap()
                .timeframe,
            "D1"
        );
        assert!(Resolution::parse(MarketType::Futures, "2D").is_err());
        // `1M` 是月线，`1m` 才是 1 分钟。
        assert_eq!(
            Resolution::parse(MarketType::Futures, "1m")
                .unwrap()
                .timeframe,
            "1m"
        );
        // dukascopy 没有周/月归档。
        assert!(Resolution::parse(MarketType::Dukascopy, "1W").is_err());
        assert!(Resolution::parse(MarketType::Dukascopy, "1M").is_err());
    }

    #[test]
    fn marks_resolution_kind() {
        let week = Resolution::parse(MarketType::Futures, "1W")
            .unwrap()
            .resolution;
        assert!(week.is_week());
        assert!(!week.is_intraday());
        assert!(!week.is_day());

        let month = Resolution::parse(MarketType::Futures, "1M")
            .unwrap()
            .resolution;
        assert!(month.is_month());

        let day = Resolution::parse(MarketType::Futures, "3D")
            .unwrap()
            .resolution;
        assert!(day.is_day());
        assert!(!day.is_intraday());

        let hour = Resolution::parse(MarketType::Futures, "60")
            .unwrap()
            .resolution;
        assert!(hour.is_intraday());
    }

    #[test]
    fn dedupes_and_sorts_supported_resolutions() {
        let tfs = vec![
            "1d".to_string(),
            "1h".to_string(),
            "M1".to_string(),
            "1m".to_string(),
            "H1".to_string(),
            "1w".to_string(),
            "1mo".to_string(),
        ];
        let udf: Vec<&str> = timeframes_to_resolutions(&tfs)
            .into_iter()
            .map(Resolution::as_udf)
            .collect();
        assert_eq!(udf, vec!["1", "60", "1D", "1W", "1M"]);
    }
}
