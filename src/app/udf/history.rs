//! `from` / `to` / `countback` 到数据层过滤参数的转换。
//!
//! 语义对齐官方 UDF：
//! - `from` 含、`to` 不含（半开区间）；
//! - `countback` 优先于 `from`，一旦出现 `from` 应被忽略；
//! - 窗口为空时用 `next_time_anchor` 回查「过去最近一根」。

/// UDF 单次最多返回的 bar 数（与 `/api/klines` 上限一致）。
pub(crate) const MAX_UDF_BARS: usize = 10_000;

/// 解析并校验后的 history 时间窗。
///
/// 内部使用本地毫秒时间戳与**开区间**过滤参数，交给 [`DataLoader`]。
///
/// [`DataLoader`]: super::super::data_loader::DataLoader
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HistoryWindow {
    after: Option<i64>,
    before: Option<i64>,
    limit: Option<usize>,
}

impl HistoryWindow {
    /// 校验 UDF 的秒级 `from` / `to` / `countback`。
    ///
    /// `countback` 存在时 `from` 完全不参与计算，因此也不校验 `from`。
    pub(crate) fn new(
        from: Option<i64>,
        to: Option<i64>,
        countback: Option<usize>,
    ) -> Result<Self, HistoryError> {
        let countback = countback.map(|n| n.clamp(1, MAX_UDF_BARS));
        let to = to.unwrap_or(0);
        if to < 0 {
            return Err(HistoryError::Negative);
        }
        // UDF 的 `to` 是右开区间，数据层的 `before` 也是开区间。
        let before = if to > 0 {
            Some(to.checked_mul(1_000).ok_or(HistoryError::ToOutOfRange)?)
        } else {
            None
        };
        let after = if countback.is_some() {
            None
        } else {
            let from = from.unwrap_or(0);
            if from < 0 {
                return Err(HistoryError::Negative);
            }
            if from > 0 && to > 0 && from > to {
                return Err(HistoryError::Reversed);
            }
            if from > 0 {
                Some(
                    from.checked_mul(1_000)
                        .and_then(|v| v.checked_sub(1))
                        .ok_or(HistoryError::FromOutOfRange)?,
                )
            } else {
                None
            }
        };
        Ok(Self {
            after,
            before,
            limit: Some(countback.unwrap_or(MAX_UDF_BARS)),
        })
    }

    /// 传给数据层的 `(before, after, limit)`（毫秒，开区间）。
    pub(crate) fn loader_args(self) -> (Option<i64>, Option<i64>, Option<usize>) {
        (self.before, self.after, self.limit)
    }

    /// 回查「过去最近一根」的上界（毫秒，开区间）。
    ///
    /// 优先窗口下界（`from`），否则退回窗口上界（`to`）；都没有时没有可回查的范围。
    pub(crate) fn next_time_anchor(self) -> Option<i64> {
        match self.after {
            Some(after) => after.checked_add(1),
            None => self.before,
        }
    }
}

/// history 参数错误。
#[derive(Debug, thiserror::Error)]
pub(crate) enum HistoryError {
    /// `from` / `to` 为负数。
    #[error("from/to must be >= 0")]
    Negative,
    /// `from` 晚于 `to`。
    #[error("from > to")]
    Reversed,
    /// `from` 秒转毫秒溢出。
    #[error("from is out of range")]
    FromOutOfRange,
    /// `to` 秒转毫秒溢出。
    #[error("to is out of range")]
    ToOutOfRange,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_negative_and_reversed_ranges() {
        assert!(matches!(
            HistoryWindow::new(Some(-1), None, None),
            Err(HistoryError::Negative)
        ));
        assert!(matches!(
            HistoryWindow::new(Some(10), Some(5), None),
            Err(HistoryError::Reversed)
        ));
    }

    #[test]
    fn defaults_limit_and_clamps_countback() {
        let (_, _, limit) = HistoryWindow::new(None, None, None).unwrap().loader_args();
        assert_eq!(limit, Some(MAX_UDF_BARS));

        let (_, _, limit) = HistoryWindow::new(None, None, Some(0))
            .unwrap()
            .loader_args();
        assert_eq!(limit, Some(1));

        let (_, _, limit) = HistoryWindow::new(None, None, Some(MAX_UDF_BARS + 1))
            .unwrap()
            .loader_args();
        assert_eq!(limit, Some(MAX_UDF_BARS));
    }

    #[test]
    fn converts_seconds_to_half_open_window() {
        let (before, after, _) = HistoryWindow::new(Some(10), Some(20), None)
            .unwrap()
            .loader_args();
        assert_eq!(after, Some(9_999));
        assert_eq!(before, Some(20_000));
    }

    #[test]
    fn countback_ignores_from_entirely() {
        // from 晚于 to 也不报错：官方规定 countback 存在时忽略 from。
        let (before, after, limit) = HistoryWindow::new(Some(20), Some(10), Some(2))
            .unwrap()
            .loader_args();
        assert_eq!(after, None);
        assert_eq!(before, Some(10_000));
        assert_eq!(limit, Some(2));
    }

    #[test]
    fn next_time_anchor_points_before_the_window() {
        let window = HistoryWindow::new(Some(10), Some(20), None).unwrap();
        assert_eq!(window.next_time_anchor(), Some(10_000));

        let window = HistoryWindow::new(None, Some(20), None).unwrap();
        assert_eq!(window.next_time_anchor(), Some(20_000));

        let window = HistoryWindow::new(None, None, None).unwrap();
        assert_eq!(window.next_time_anchor(), None);
    }
}
