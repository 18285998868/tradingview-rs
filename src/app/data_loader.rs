//! 行情数据加载：binance zip → K 线列 → 内存缓存。

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use thiserror::Error;
use tracing::instrument;

use crate::utils::{has_data_files, has_kline_files, timeframe_order};

/// 单个数据集超过该行数时打告警（内存占用提示）。
const LARGE_DATASET_ROWS: usize = 500_000;

/// 行情市场类型。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MarketType {
    /// binance U 本位合约（`{dir}/binance/data/futures/um/daily/klines/{symbol}/{tf}/*.zip`）。
    #[default]
    Futures,
    /// binance 现货（本地暂无对应数据）。
    Spot,
    /// Dukascopy 外汇/贵金属（`{dir}/dukascopy/{symbol}/{yyyy}/{MM}/{dd}/*.bi5`，
    /// 月份 0-based：`00` = 1 月）。
    Dukascopy,
}

impl MarketType {
    /// 对外暴露的市场类型（= **本地确有数据源**的那些）。
    ///
    /// `Spot` 变体仍可被解析，但本地没有对应数据，故不对外暴露 ——
    /// 否则前端会列出一个"选了就是空图 / 404"的选项。
    pub const SUPPORTED: [MarketType; 2] = [MarketType::Futures, MarketType::Dukascopy];

    /// 数据文件扩展名（binance = zip，dukascopy = bi5）。
    fn data_ext(self) -> &'static str {
        match self {
            MarketType::Futures | MarketType::Spot => "zip",
            MarketType::Dukascopy => "bi5",
        }
    }
}

impl std::fmt::Display for MarketType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MarketType::Futures => write!(f, "futures"),
            MarketType::Spot => write!(f, "spot"),
            MarketType::Dukascopy => write!(f, "dukascopy"),
        }
    }
}

impl std::str::FromStr for MarketType {
    type Err = ParseMarketTypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("futures") {
            Ok(MarketType::Futures)
        } else if s.eq_ignore_ascii_case("spot") {
            Ok(MarketType::Spot)
        } else if s.eq_ignore_ascii_case("dukascopy") {
            Ok(MarketType::Dukascopy)
        } else {
            Err(ParseMarketTypeError::new(s))
        }
    }
}

/// `market_type` 解析失败。
#[derive(Debug, Clone, Error)]
#[error("unknown market type: {0}")]
pub struct ParseMarketTypeError(String);

impl ParseMarketTypeError {
    /// 构造解析错误。
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// 触发解析失败的原始输入。
    pub fn value(&self) -> &str {
        &self.0
    }
}

/// 行情数据错误。
#[derive(Debug, Error)]
pub enum DataError {
    /// 文件系统 IO 失败。
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    /// binance-core 读取/解析失败。
    #[error("binance reader error: {0}")]
    BinanceReader(#[from] binance_core::Error),

    /// dukascopy-core 读取/解析失败。
    #[error("dukascopy reader error: {0}")]
    DukascopyReader(#[from] dukascopy_core::ReaderError),

    /// 数据内容不完整（例如缺少必需列）。
    #[error("data error: {0}")]
    Data(String),

    /// 数据目录或数据集不存在。
    #[error("not found: {0}")]
    NotFound(String),

    /// 入参非法（如该数据源不支持此周期）——属于用户错误，映射为 400。
    #[error("invalid parameter: {0}")]
    InvalidParam(String),

    /// 后台任务失败（线程池错误 / 任务 panic）。
    #[error("background task failed: {0}")]
    Task(String),
}

/// 单根 K 线（JSON DTO）。
#[derive(Debug, Clone, Serialize)]
pub struct Kline {
    /// bar 开盘时间（毫秒时间戳）。
    pub time: i64,
    /// 开盘价。
    pub open: f64,
    /// 最高价。
    pub high: f64,
    /// 最低价。
    pub low: f64,
    /// 收盘价。
    pub close: f64,
    /// 成交量。
    pub volume: f64,
}

/// 与 bar 对齐的 6 条列（SoA 布局）。
///
/// 缓存与回测都直接消费列切片：vta 引擎要的是 `&[f64]`，若中间层存
/// `Vec<Kline>`（AoS），每次回测都要把结构体再拆成 6 份副本。
#[derive(Debug, Clone, Default)]
pub struct KlineColumns {
    /// bar 开盘时间（毫秒时间戳），升序且无重复。
    pub time: Vec<i64>,
    /// 开盘价。
    pub open: Vec<f64>,
    /// 最高价。
    pub high: Vec<f64>,
    /// 最低价。
    pub low: Vec<f64>,
    /// 收盘价。
    pub close: Vec<f64>,
    /// 成交量。
    pub volume: Vec<f64>,
}

/// 六列的原始向量，按 `time/open/high/low/close/volume` 排列。
type ColumnParts = (Vec<i64>, Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>);

impl KlineColumns {
    /// 按 bar 数预分配。
    fn with_capacity(n: usize) -> Self {
        Self {
            time: Vec::with_capacity(n),
            open: Vec::with_capacity(n),
            high: Vec::with_capacity(n),
            low: Vec::with_capacity(n),
            close: Vec::with_capacity(n),
            volume: Vec::with_capacity(n),
        }
    }

    /// 从 reader 的列向量构造：丢弃非有限值行、按时间去重（保留首行）。
    ///
    /// reader 已保证输出按 `open_time` 升序且已去重，这里再做一次是
    /// 防御性兜底（数据源换实现时不至于静默产出重复 bar）。
    fn from_parts(parts: ColumnParts) -> Self {
        let (time, open, high, low, close, volume) = parts;

        let mut cols = Self::with_capacity(time.len());
        let mut last_time: Option<i64> = None;
        for i in 0..time.len() {
            let (o, h, l, c, v) = (open[i], high[i], low[i], close[i], volume[i]);
            if !o.is_finite()
                || !h.is_finite()
                || !l.is_finite()
                || !c.is_finite()
                || !v.is_finite()
            {
                continue;
            }
            if last_time == Some(time[i]) {
                continue;
            }
            last_time = Some(time[i]);
            cols.time.push(time[i]);
            cols.open.push(o);
            cols.high.push(h);
            cols.low.push(l);
            cols.close.push(c);
            cols.volume.push(v);
        }
        cols
    }

    /// bar 数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.time.len()
    }

    /// 是否没有 bar。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.time.is_empty()
    }

    /// 取区间内的列副本（越界自动收敛到合法范围）。
    #[must_use]
    pub fn slice(&self, range: Range<usize>) -> Self {
        let (start, end) = self.bounds(range);
        Self {
            time: self.time[start..end].to_vec(),
            open: self.open[start..end].to_vec(),
            high: self.high[start..end].to_vec(),
            low: self.low[start..end].to_vec(),
            close: self.close[start..end].to_vec(),
            volume: self.volume[start..end].to_vec(),
        }
    }

    /// 取区间内的 bar 并转成 JSON DTO。
    #[must_use]
    pub fn to_klines(&self, range: Range<usize>) -> Vec<Kline> {
        let (start, end) = self.bounds(range);
        let mut out = Vec::with_capacity(end - start);
        for i in start..end {
            out.push(Kline {
                time: self.time[i],
                open: self.open[i],
                high: self.high[i],
                low: self.low[i],
                close: self.close[i],
                volume: self.volume[i],
            });
        }
        out
    }

    /// 把区间收敛到 `[0, len]` 内且 `start <= end`。
    fn bounds(&self, range: Range<usize>) -> (usize, usize) {
        let len = self.len();
        let start = range.start.min(len);
        let end = range.end.clamp(start, len);
        (start, end)
    }
}

impl TryFrom<binance_core::Table> for KlineColumns {
    type Error = DataError;

    fn try_from(table: binance_core::Table) -> Result<Self, Self::Error> {
        fn column<'a, T>(
            table: &'a binance_core::Table,
            name: &str,
            get: impl Fn(&'a binance_core::Table, &str) -> Option<&'a [T]>,
        ) -> Result<&'a [T], DataError> {
            get(table, name)
                .ok_or_else(|| DataError::Data(format!("kline table is missing `{name}` column")))
        }

        let time = column(&table, "open_time", binance_core::Table::i64)?.to_vec();
        let open = column(&table, "open", binance_core::Table::f64)?.to_vec();
        let high = column(&table, "high", binance_core::Table::f64)?.to_vec();
        let low = column(&table, "low", binance_core::Table::f64)?.to_vec();
        let close = column(&table, "close", binance_core::Table::f64)?.to_vec();
        let volume = column(&table, "volume", binance_core::Table::f64)?.to_vec();
        Ok(Self::from_parts((time, open, high, low, close, volume)))
    }
}

impl From<dukascopy_core::KlineColumns> for KlineColumns {
    fn from(raw: dukascopy_core::KlineColumns) -> Self {
        let parts = raw.into_parts();
        Self::from_parts((
            parts.time,
            parts.open,
            parts.high,
            parts.low,
            parts.close,
            // Dukascopy stores raw `u32` volume units; this layer uses `f64`.
            parts.volume.into_iter().map(f64::from).collect(),
        ))
    }
}

/// 计算时间过滤 + 限长后的 bar 区间 `[start, end)`。
///
/// `time` 必须升序（reader 保证）。`before`/`after` 是开区间过滤，
/// `limit` 取窗口内**最后** N 根（与历史行为一致）。
fn time_window(
    time: &[i64],
    before: Option<i64>,
    after: Option<i64>,
    limit: Option<usize>,
) -> Range<usize> {
    let mut start = match after {
        Some(t) => time.partition_point(|&x| x <= t),
        None => 0,
    };
    let mut end = match before {
        Some(t) => time.partition_point(|&x| x < t),
        None => time.len(),
    };
    end = end.max(start);
    if let Some(l) = limit {
        // 取窗口末尾 N 根：起点右移，而不是把终点左移。
        start = end.saturating_sub(l).max(start);
    }
    start..end
}

/// 品种目录下是否确实有可读的 kline 文件（空目录不算）。
///
/// binance 的 zip 躺在周期子目录里，dukascopy 的 `.bi5` 嵌在 `年/月/日` 多层目录里。
fn pair_has_data(market_type: MarketType, pair_dir: &Path) -> bool {
    match market_type {
        MarketType::Dukascopy => has_data_files(pair_dir, market_type.data_ext()),
        _ => std::fs::read_dir(pair_dir)
            .map(|rd| {
                rd.flatten()
                    .any(|e| e.path().is_dir() && has_kline_files(&e.path()))
            })
            .unwrap_or(false),
    }
}

/// 扫描 dukascopy 品种目录，返回**确有 `.bi5` 文件**的周期（按 [`Timeframe::ALL`] 顺序）。
///
/// Dukascopy 把聚合粒度编码在文件名里（`BID_candles_hour_1.bi5`），目录结构不区分
/// 周期。只要目录里存在任意 `.bi5` 就宣称支持 M1/H1/D1，会让前端默认落到空图；
/// 这里按 reader 的文件命名逐个核对，三个都找到就提前结束遍历。
///
/// [`Timeframe::ALL`]: dukascopy_core::Timeframe::ALL
fn dukascopy_timeframes_with_files(pair_dir: &Path) -> Vec<dukascopy_core::Timeframe> {
    let timeframes = dukascopy_core::Timeframe::ALL;
    // 归档文件名 = `{BID|ASK}_{candles_min_1|candles_hour_1|candles_day_1}.bi5`
    let expected: Vec<[String; 2]> = timeframes
        .iter()
        .map(|timeframe| {
            let stem = timeframe.file_stem();
            dukascopy_core::PriceType::ALL
                .map(|price_type| format!("{}_{stem}.bi5", price_type.prefix()))
        })
        .collect();
    let mut found = [false; dukascopy_core::Timeframe::ALL.len()];

    fn walk(dir: &Path, expected: &[[String; 2]], found: &mut [bool]) {
        if found.iter().all(|hit| *hit) {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, expected, found);
            } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                for (idx, names) in expected.iter().enumerate() {
                    if !found[idx] && names.iter().any(|candidate| candidate.as_str() == name) {
                        found[idx] = true;
                    }
                }
            }
        }
    }
    walk(pair_dir, &expected, &mut found);

    timeframes
        .into_iter()
        .zip(found)
        .filter_map(|(timeframe, hit)| hit.then_some(timeframe))
        .collect()
}

/// 缓存键：市场类型 + 交易对 + 周期。
type CacheKey = (MarketType, String, String);

/// 行情数据加载器。
///
/// 读到的列集以 `Arc` 共享并进 moka 缓存，命中时零拷贝。
pub struct DataLoader {
    data_dir: PathBuf,
    klines_cache: moka::sync::Cache<CacheKey, Arc<KlineColumns>>,
}

impl DataLoader {
    /// 构造加载器。
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            klines_cache: moka::sync::Cache::builder()
                .max_capacity(200)
                .time_to_live(Duration::from_secs(300))
                .build(),
        }
    }

    /// 数据源根目录（按 reader 的归档约定给出**全部**候选）。
    ///
    /// - Futures：`daily` 与 `monthly` 两棵归档。reader 会合并两处（同一 UTC 日
    ///   日线优先），发现层也必须同时看：只扫 `daily` 会漏掉仅存在于月度归档的
    ///   品种（已下架合约）和周期（`3d`/`1w`/`1mo`），而 reader 本身能读。
    /// - Dukascopy：`{dir}/dukascopy`（周期不是目录，而是文件名里的聚合粒度）。
    /// - Spot：本地暂无对应数据源，返回空，调用方按空/404 处理。
    fn bases(&self, market_type: MarketType) -> Vec<PathBuf> {
        match market_type {
            MarketType::Futures => ["daily", "monthly"]
                .into_iter()
                .map(|period| {
                    self.data_dir
                        .join("binance")
                        .join("data")
                        .join("futures")
                        .join("um")
                        .join(period)
                        .join("klines")
                })
                .collect(),
            MarketType::Spot => Vec::new(),
            MarketType::Dukascopy => vec![self.data_dir.join("dukascopy")],
        }
    }

    /// 列出可用交易对（字典序去重）。
    ///
    /// binance 取 `daily ∪ monthly`：只有月度归档的品种（例如已下架合约）也要出现。
    pub fn list_pairs(&self, market_type: MarketType) -> Result<Vec<String>, DataError> {
        let mut pairs = Vec::new();
        for base in self.bases(market_type) {
            if !base.exists() {
                continue;
            }
            for entry in std::fs::read_dir(&base)?.flatten() {
                let path = entry.path();
                if !path.is_dir() || !pair_has_data(market_type, &path) {
                    continue;
                }
                if let Ok(name) = entry.file_name().into_string() {
                    pairs.push(name);
                }
            }
        }
        pairs.sort();
        pairs.dedup();
        Ok(pairs)
    }

    /// 列出某交易对下**确有数据文件**的周期（按周期大小排序）。
    pub fn list_timeframes(
        &self,
        market_type: MarketType,
        pair: &str,
    ) -> Result<Vec<String>, DataError> {
        let mut timeframes: Vec<String> = match market_type {
            // 周期编码在文件名（`BID_candles_hour_1.bi5`），只列真实存在的粒度：
            // 全列 M1/H1/D1 会让前端默认落到空图（本地往往只下了一部分粒度）。
            MarketType::Dukascopy => {
                let Some(base) = self.bases(market_type).into_iter().next() else {
                    return Ok(Vec::new());
                };
                dukascopy_timeframes_with_files(&base.join(pair))
                    .into_iter()
                    .map(|timeframe| timeframe.as_str().to_string())
                    .collect()
            }
            // binance：周期是子目录，子目录里有 zip 即为"有数据"；日/月两处取并集。
            _ => {
                let mut found = Vec::new();
                for base in self.bases(market_type) {
                    let pair_dir = base.join(pair);
                    if !pair_dir.exists() {
                        continue;
                    }
                    for entry in std::fs::read_dir(&pair_dir)?.flatten() {
                        let path = entry.path();
                        if !path.is_dir() || !has_kline_files(&path) {
                            continue;
                        }
                        if let Ok(name) = entry.file_name().into_string() {
                            found.push(name);
                        }
                    }
                }
                found
            }
        };
        timeframes.sort_by_key(|a| timeframe_order(a));
        timeframes.dedup();
        Ok(timeframes)
    }

    /// 取过滤后的 K 线（JSON DTO）。
    ///
    /// `before`/`after` 为开区间过滤，`limit` 取窗口内最后 N 根。
    pub fn load_klines_filtered(
        &self,
        market_type: MarketType,
        pair: &str,
        timeframe: &str,
        before: Option<i64>,
        after: Option<i64>,
        limit: Option<usize>,
    ) -> Result<Vec<Kline>, DataError> {
        let all = self.load_all_klines(market_type, pair, timeframe)?;
        let range = time_window(&all.time, before, after, limit);
        Ok(all.to_klines(range))
    }

    /// 取过滤后的列集（供回测直接消费）。
    ///
    /// 与 [`DataLoader::load_klines_filtered`] 的区别：不构造 JSON DTO，
    /// 直接给回测引擎喂列切片，省掉一次结构体拷贝。
    pub fn load_columns_filtered(
        &self,
        market_type: MarketType,
        pair: &str,
        timeframe: &str,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<Arc<KlineColumns>, DataError> {
        let all = self.load_all_klines(market_type, pair, timeframe)?;
        let range = time_window(&all.time, before, after, None);
        if range.start == 0 && range.end == all.len() {
            return Ok(all);
        }
        Ok(Arc::new(all.slice(range)))
    }

    /// 读取全部 K 线列（带缓存）。
    #[instrument(skip(self))]
    fn load_all_klines(
        &self,
        market_type: MarketType,
        pair: &str,
        timeframe: &str,
    ) -> Result<Arc<KlineColumns>, DataError> {
        let key = (market_type, pair.to_owned(), timeframe.to_owned());
        if let Some(cached) = self.klines_cache.get(&key) {
            tracing::debug!(?key, "cache hit");
            return Ok(cached);
        }
        tracing::debug!(?key, "cache miss — reading kline files");

        // 周期合法性由数据源裁定：不同源的可用周期不同（binance 1m..1mo，
        // dukascopy M1/H1/D1，别名 1m/1h/1d 由 reader 归一），由对应 reader
        // 判定才能给出"该源支持哪些"的准确信息；用户输错属于 400 而非 500。
        match market_type {
            MarketType::Dukascopy => {
                timeframe
                    .parse::<dukascopy_core::Timeframe>()
                    .map_err(|e| DataError::InvalidParam(e.to_string()))?;
            }
            _ => {
                timeframe
                    .parse::<binance_core::Interval>()
                    .map_err(DataError::InvalidParam)?;
            }
        }

        let ext = market_type.data_ext();

        // binance：周期是子目录，日/月两棵归档都要探测（reader 会合并两处）；
        // dukascopy：品种目录下直接是年/月/日。
        let probe_dirs: Vec<PathBuf> = self
            .bases(market_type)
            .into_iter()
            .map(|base| match market_type {
                MarketType::Dukascopy => base.join(pair),
                _ => base.join(pair).join(timeframe),
            })
            .collect();
        if probe_dirs.is_empty() {
            tracing::warn!("market not backed by a data reader yet");
            return Err(DataError::NotFound(format!(
                "market not supported: {pair}/{timeframe}"
            )));
        }
        if !probe_dirs
            .iter()
            .any(|dir| dir.exists() && has_data_files(dir, ext))
        {
            let paths: Vec<String> = probe_dirs
                .iter()
                .map(|dir| dir.display().to_string())
                .collect();
            tracing::warn!(?paths, ext, "no kline data files");
            return Err(DataError::NotFound(format!(
                "no {ext} data files in {}",
                probe_dirs[0].display()
            )));
        }

        let cols = match market_type {
            MarketType::Dukascopy => {
                let timeframe = timeframe
                    .parse::<dukascopy_core::Timeframe>()
                    .map_err(|e| DataError::InvalidParam(e.to_string()))?;
                let cfg = dukascopy_core::DataConfig::new(&self.data_dir, pair, timeframe)?;
                let raw = dukascopy_core::load_columns(&cfg)?;
                KlineColumns::from(raw)
            }
            _ => {
                let market = match market_type {
                    MarketType::Spot => binance_core::Market::Spot,
                    _ => binance_core::Market::Um,
                };
                let interval: binance_core::Interval =
                    timeframe.parse().map_err(DataError::InvalidParam)?;
                let cfg = binance_core::Config::builder(
                    self.data_dir.join("binance"),
                    binance_core::Dataset::Klines,
                    pair,
                )
                .market(market)
                .interval(interval)
                .build()?;
                KlineColumns::try_from(binance_core::read(&cfg)?.table)?
            }
        };
        if cols.is_empty() {
            return Err(DataError::NotFound(format!(
                "no klines loaded for {pair}/{timeframe}"
            )));
        }
        if cols.len() > LARGE_DATASET_ROWS {
            tracing::warn!(
                rows = cols.len(),
                "large kline dataset loaded — consider using before/after filters to reduce memory"
            );
        }

        tracing::debug!(count = cols.len(), "klines after clean");
        let result = Arc::new(cols);
        self.klines_cache.insert(key, result.clone());
        Ok(result)
    }

    /// 清空 K 线缓存。
    pub fn invalidate_cache(&self) {
        self.klines_cache.invalidate_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::TempDir;

    fn binance_kline_dir(dir: &Path, period: &str, pair: &str, timeframe: &str) -> PathBuf {
        dir.join("binance")
            .join("data")
            .join("futures")
            .join("um")
            .join(period)
            .join("klines")
            .join(pair)
            .join(timeframe)
    }

    fn binance_tf_dir(dir: &Path, pair: &str, timeframe: &str) -> PathBuf {
        binance_kline_dir(dir, "daily", pair, timeframe)
    }

    /// 2024-01-01T00:00:00Z；所有 kline fixture 都落在这天的文件周期内。
    const BASE_MS: i64 = 1_704_067_200_000;
    const HOUR_MS: i64 = 3_600_000;

    fn write_test_zip_in(
        dir: &Path,
        period: &str,
        pair: &str,
        timeframe: &str,
        name: &str,
        rows: &[(i64, f64, f64, f64, f64, f64)],
    ) {
        let tf_dir = binance_kline_dir(dir, period, pair, timeframe);
        std::fs::create_dir_all(&tf_dir).unwrap();
        let mut csv = String::new();
        for r in rows {
            // 读入的 fixture 都是 1h bar：close_time 必须是 open + 1h - 1ms，
            // 否则新版 reader 会按 `BadRowPolicy::Fail` 直接报错。
            csv.push_str(&format!(
                "{},{},{},{},{},{},{},0,0,0,0,0\n",
                r.0,
                r.1,
                r.2,
                r.3,
                r.4,
                r.5,
                r.0 + HOUR_MS - 1
            ));
        }
        let zip_path = tf_dir.join(name);
        let file = std::fs::File::create(&zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let csv_name = name.replace(".zip", ".csv");
        zip.start_file::<&str, ()>(&csv_name, zip::write::FileOptions::default())
            .unwrap();
        use std::io::Write as _;
        zip.write_all(csv.as_bytes()).unwrap();
        zip.finish().unwrap();
    }

    fn write_test_zip(
        dir: &Path,
        pair: &str,
        timeframe: &str,
        name: &str,
        rows: &[(i64, f64, f64, f64, f64, f64)],
    ) {
        write_test_zip_in(dir, "daily", pair, timeframe, name, rows);
    }

    fn test_rows() -> Vec<(i64, f64, f64, f64, f64, f64)> {
        vec![
            (BASE_MS, 100.0, 105.0, 95.0, 102.0, 10.0),
            (BASE_MS + HOUR_MS, 102.0, 107.0, 97.0, 104.0, 20.0),
            (BASE_MS + 2 * HOUR_MS, 104.0, 109.0, 99.0, 106.0, 30.0),
        ]
    }

    #[test]
    fn test_market_type_default() {
        assert_eq!(MarketType::default(), MarketType::Futures);
    }

    #[test]
    fn test_market_type_display() {
        assert_eq!(MarketType::Futures.to_string(), "futures");
        assert_eq!(MarketType::Spot.to_string(), "spot");
    }

    #[test]
    fn test_market_type_from_str() {
        assert_eq!(
            "futures".parse::<MarketType>().unwrap(),
            MarketType::Futures
        );
        assert_eq!("spot".parse::<MarketType>().unwrap(), MarketType::Spot);
        assert!("invalid".parse::<MarketType>().is_err());
    }

    #[test]
    fn test_market_type_from_str_is_case_insensitive() {
        assert_eq!(
            "FUTURES".parse::<MarketType>().unwrap(),
            MarketType::Futures
        );
        assert_eq!("Spot".parse::<MarketType>().unwrap(), MarketType::Spot);
    }

    #[test]
    fn test_parse_market_type_error_display() {
        let e = ParseMarketTypeError::new("bad");
        assert_eq!(e.to_string(), "unknown market type: bad");
        assert_eq!(e.value(), "bad");
    }

    #[test]
    fn test_list_pairs_empty_dir() {
        let dir = TempDir::new().unwrap();
        let loader = DataLoader::new(dir.path());
        let pairs = loader.list_pairs(MarketType::Futures).unwrap();
        assert!(pairs.is_empty());
    }

    #[test]
    fn test_list_pairs_with_data() {
        let dir = TempDir::new().unwrap();
        write_test_zip(
            dir.path(),
            "BTCUSDT",
            "1h",
            "BTCUSDT-1h-2024-01-01.zip",
            &test_rows(),
        );
        write_test_zip(
            dir.path(),
            "ETHUSDT",
            "1h",
            "ETHUSDT-1h-2024-01-01.zip",
            &test_rows(),
        );

        let loader = DataLoader::new(dir.path());
        let pairs = loader.list_pairs(MarketType::Futures).unwrap();
        assert_eq!(pairs, vec!["BTCUSDT", "ETHUSDT"]);
    }

    #[test]
    fn test_list_timeframes_only_with_zip() {
        let dir = TempDir::new().unwrap();
        write_test_zip(
            dir.path(),
            "BTCUSDT",
            "1h",
            "BTCUSDT-1h-2024-01-01.zip",
            &test_rows(),
        );
        write_test_zip(
            dir.path(),
            "BTCUSDT",
            "1d",
            "BTCUSDT-1d-2024-01-01.zip",
            &test_rows(),
        );
        let empty_dir = binance_tf_dir(dir.path(), "BTCUSDT", "1w");
        std::fs::create_dir_all(&empty_dir).unwrap();

        let loader = DataLoader::new(dir.path());
        let tfs = loader
            .list_timeframes(MarketType::Futures, "BTCUSDT")
            .unwrap();
        assert_eq!(tfs, vec!["1h", "1d"]);
    }

    #[test]
    fn test_monthly_only_pair_and_timeframe_are_visible() {
        let dir = TempDir::new().unwrap();
        // 真实数据里 FTTUSDT / LUNAUSDT 只有月度归档（日线归档没有该品种目录）。
        write_test_zip_in(
            dir.path(),
            "monthly",
            "FTTUSDT",
            "1h",
            "FTTUSDT-1h-2024-01.zip",
            &test_rows(),
        );
        // 只有空周期目录的品种不算"有数据"。
        std::fs::create_dir_all(binance_kline_dir(dir.path(), "monthly", "EMPTYUSDT", "1h"))
            .unwrap();

        let loader = DataLoader::new(dir.path());
        assert_eq!(
            loader.list_pairs(MarketType::Futures).unwrap(),
            vec!["FTTUSDT"]
        );
        assert_eq!(
            loader
                .list_timeframes(MarketType::Futures, "FTTUSDT")
                .unwrap(),
            vec!["1h"]
        );

        let klines = loader
            .load_klines_filtered(MarketType::Futures, "FTTUSDT", "1h", None, None, None)
            .unwrap();
        assert_eq!(klines.len(), 3);
    }

    #[test]
    fn test_daily_and_monthly_are_unioned() {
        let dir = TempDir::new().unwrap();
        write_test_zip(
            dir.path(),
            "BTCUSDT",
            "1h",
            "BTCUSDT-1h-2024-01-01.zip",
            &test_rows(),
        );
        // 月度归档覆盖整月；reader 合并时日线优先、月线只补日线缺口。
        write_test_zip_in(
            dir.path(),
            "monthly",
            "BTCUSDT",
            "1h",
            "BTCUSDT-1h-2024-01.zip",
            &[(BASE_MS + 24 * HOUR_MS, 104.0, 109.0, 99.0, 106.0, 30.0)],
        );

        let loader = DataLoader::new(dir.path());
        // 同一品种/周期在两处出现，对外只列一次。
        assert_eq!(
            loader.list_pairs(MarketType::Futures).unwrap(),
            vec!["BTCUSDT"]
        );
        assert_eq!(
            loader
                .list_timeframes(MarketType::Futures, "BTCUSDT")
                .unwrap(),
            vec!["1h"]
        );

        let klines = loader
            .load_klines_filtered(MarketType::Futures, "BTCUSDT", "1h", None, None, None)
            .unwrap();
        assert_eq!(klines.len(), 4);
        assert_eq!(klines[0].time, BASE_MS);
        assert_eq!(klines[3].time, BASE_MS + 24 * HOUR_MS);
    }

    #[test]
    fn test_spot_not_supported_yet() {
        let dir = TempDir::new().unwrap();
        let loader = DataLoader::new(dir.path());
        assert!(loader.list_pairs(MarketType::Spot).unwrap().is_empty());
        assert!(
            loader
                .load_klines_filtered(MarketType::Spot, "BTCUSDT", "1h", None, None, None)
                .is_err()
        );
    }

    #[test]
    fn test_load_klines_filtered_all() {
        let dir = TempDir::new().unwrap();
        write_test_zip(
            dir.path(),
            "BTCUSDT",
            "1h",
            "BTCUSDT-1h-2024-01-01.zip",
            &test_rows(),
        );

        let loader = DataLoader::new(dir.path());
        let klines = loader
            .load_klines_filtered(MarketType::Futures, "BTCUSDT", "1h", None, None, None)
            .unwrap();
        assert_eq!(klines.len(), 3);
    }

    #[test]
    fn test_load_klines_filtered_before() {
        let dir = TempDir::new().unwrap();
        write_test_zip(
            dir.path(),
            "BTCUSDT",
            "1h",
            "BTCUSDT-1h-2024-01-01.zip",
            &test_rows(),
        );

        let loader = DataLoader::new(dir.path());
        let klines = loader
            .load_klines_filtered(
                MarketType::Futures,
                "BTCUSDT",
                "1h",
                Some(BASE_MS + HOUR_MS + HOUR_MS / 2),
                None,
                None,
            )
            .unwrap();
        assert_eq!(klines.len(), 2);
        assert_eq!(klines[0].time, BASE_MS);
    }

    #[test]
    fn test_load_klines_filtered_after() {
        let dir = TempDir::new().unwrap();
        write_test_zip(
            dir.path(),
            "BTCUSDT",
            "1h",
            "BTCUSDT-1h-2024-01-01.zip",
            &test_rows(),
        );

        let loader = DataLoader::new(dir.path());
        let klines = loader
            .load_klines_filtered(
                MarketType::Futures,
                "BTCUSDT",
                "1h",
                None,
                Some(BASE_MS + HOUR_MS / 2),
                None,
            )
            .unwrap();
        assert_eq!(klines.len(), 2);
        assert_eq!(klines[0].time, BASE_MS + HOUR_MS);
    }

    #[test]
    fn test_load_klines_filtered_limit() {
        let dir = TempDir::new().unwrap();
        write_test_zip(
            dir.path(),
            "BTCUSDT",
            "1h",
            "BTCUSDT-1h-2024-01-01.zip",
            &test_rows(),
        );

        let loader = DataLoader::new(dir.path());
        let klines = loader
            .load_klines_filtered(MarketType::Futures, "BTCUSDT", "1h", None, None, Some(2))
            .unwrap();
        assert_eq!(klines.len(), 2);
        assert_eq!(klines[0].time, BASE_MS + HOUR_MS);
    }

    #[test]
    fn test_load_klines_filtered_limit_exceeds_available() {
        let dir = TempDir::new().unwrap();
        write_test_zip(
            dir.path(),
            "BTCUSDT",
            "1h",
            "BTCUSDT-1h-2024-01-01.zip",
            &test_rows(),
        );

        let loader = DataLoader::new(dir.path());
        let klines = loader
            .load_klines_filtered(MarketType::Futures, "BTCUSDT", "1h", None, None, Some(999))
            .unwrap();
        assert_eq!(klines.len(), 3);
    }

    #[test]
    fn test_load_klines_filtered_empty_window() {
        let dir = TempDir::new().unwrap();
        write_test_zip(
            dir.path(),
            "BTCUSDT",
            "1h",
            "BTCUSDT-1h-2024-01-01.zip",
            &test_rows(),
        );

        let loader = DataLoader::new(dir.path());
        // before <= after ⇒ 窗口为空
        let klines = loader
            .load_klines_filtered(
                MarketType::Futures,
                "BTCUSDT",
                "1h",
                Some(BASE_MS + HOUR_MS),
                Some(BASE_MS),
                None,
            )
            .unwrap();
        assert!(klines.is_empty());
    }

    #[test]
    fn test_columns_skip_nan() {
        // 新版 binance reader 对非有限值直接报 BadRow，NaN 防护留在本层：
        // 用 `from_parts` 构造含 NaN 的列，验证下游仍会丢弃该行。
        let cols = KlineColumns::from_parts((
            vec![BASE_MS, BASE_MS + HOUR_MS, BASE_MS + 2 * HOUR_MS],
            vec![100.0, f64::NAN, 104.0],
            vec![105.0, 107.0, 109.0],
            vec![95.0, 97.0, 99.0],
            vec![102.0, 104.0, 106.0],
            vec![10.0, 20.0, 30.0],
        ));
        assert_eq!(cols.len(), 2);
        assert_eq!(cols.time, vec![BASE_MS, BASE_MS + 2 * HOUR_MS]);
    }

    #[test]
    fn test_columns_dedup_keeps_first() {
        // 新版 binance reader 在加载阶段就拒绝重复 open_time，去重防护留在本层。
        let cols = KlineColumns::from_parts((
            vec![BASE_MS, BASE_MS, BASE_MS + HOUR_MS],
            vec![100.0, 102.0, 104.0],
            vec![105.0, 107.0, 109.0],
            vec![95.0, 97.0, 99.0],
            vec![102.0, 104.0, 106.0],
            vec![10.0, 20.0, 30.0],
        ));
        assert_eq!(cols.len(), 2);
        assert_eq!(cols.close, vec![102.0, 106.0]);
    }

    #[test]
    fn test_load_klines_cached() {
        let dir = TempDir::new().unwrap();
        write_test_zip(
            dir.path(),
            "BTCUSDT",
            "1h",
            "BTCUSDT-1h-2024-01-01.zip",
            &test_rows(),
        );

        let loader = DataLoader::new(dir.path());

        let klines1 = loader
            .load_klines_filtered(MarketType::Futures, "BTCUSDT", "1h", None, None, None)
            .unwrap();
        assert_eq!(klines1.len(), 3);

        std::fs::remove_dir_all(binance_tf_dir(dir.path(), "BTCUSDT", "1h")).unwrap();
        let klines2 = loader
            .load_klines_filtered(MarketType::Futures, "BTCUSDT", "1h", None, None, None)
            .unwrap();
        assert_eq!(klines2.len(), 3);
    }

    #[test]
    fn test_load_klines_not_found() {
        let dir = TempDir::new().unwrap();
        let loader = DataLoader::new(dir.path());
        let result =
            loader.load_klines_filtered(MarketType::Futures, "NONEXISTENT", "1h", None, None, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalidate_cache() {
        let dir = TempDir::new().unwrap();
        write_test_zip(
            dir.path(),
            "BTCUSDT",
            "1h",
            "BTCUSDT-1h-2024-01-01.zip",
            &test_rows(),
        );

        let loader = DataLoader::new(dir.path());
        loader
            .load_klines_filtered(MarketType::Futures, "BTCUSDT", "1h", None, None, None)
            .unwrap();
        loader.invalidate_cache();

        std::fs::remove_dir_all(binance_tf_dir(dir.path(), "BTCUSDT", "1h")).unwrap();
        let result =
            loader.load_klines_filtered(MarketType::Futures, "BTCUSDT", "1h", None, None, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_load_columns_filtered_matches_klines() {
        let dir = TempDir::new().unwrap();
        write_test_zip(
            dir.path(),
            "BTCUSDT",
            "1h",
            "BTCUSDT-1h-2024-01-01.zip",
            &test_rows(),
        );

        let loader = DataLoader::new(dir.path());
        let cols = loader
            .load_columns_filtered(
                MarketType::Futures,
                "BTCUSDT",
                "1h",
                None,
                Some(BASE_MS + HOUR_MS / 2),
            )
            .unwrap();
        assert_eq!(cols.len(), 2);
        assert_eq!(cols.time, vec![BASE_MS + HOUR_MS, BASE_MS + 2 * HOUR_MS]);
        assert_eq!(cols.close, vec![104.0, 106.0]);
        assert_eq!(cols.len(), cols.open.len());
        assert_eq!(cols.len(), cols.volume.len());
    }

    #[test]
    fn test_load_columns_filtered_unfiltered_shares_cache() {
        let dir = TempDir::new().unwrap();
        write_test_zip(
            dir.path(),
            "BTCUSDT",
            "1h",
            "BTCUSDT-1h-2024-01-01.zip",
            &test_rows(),
        );

        let loader = DataLoader::new(dir.path());
        let cols = loader
            .load_columns_filtered(MarketType::Futures, "BTCUSDT", "1h", None, None)
            .unwrap();
        assert_eq!(cols.len(), 3);
    }

    #[test]
    fn test_time_window_bounds() {
        let time = [10i64, 20, 30, 40];
        assert_eq!(time_window(&time, None, None, None), 0..4);
        assert_eq!(time_window(&time, Some(30), None, None), 0..2);
        assert_eq!(time_window(&time, None, Some(20), None), 2..4);
        assert_eq!(time_window(&time, Some(30), Some(10), None), 1..2);
        assert_eq!(time_window(&time, None, None, Some(2)), 2..4);
        assert_eq!(time_window(&time, Some(10), None, None), 0..0);
    }

    #[test]
    fn test_concurrent_cache_access() {
        let dir = TempDir::new().unwrap();
        write_test_zip(
            dir.path(),
            "BTCUSDT",
            "1h",
            "BTCUSDT-1h-2024-01-01.zip",
            &test_rows(),
        );

        let loader = Arc::new(DataLoader::new(dir.path()));

        std::thread::scope(|s| {
            for _ in 0..10 {
                let loader = Arc::clone(&loader);
                s.spawn(move || {
                    let klines = loader
                        .load_klines_filtered(
                            MarketType::Futures,
                            "BTCUSDT",
                            "1h",
                            None,
                            None,
                            None,
                        )
                        .unwrap();
                    assert_eq!(klines.len(), 3);
                });
            }
        });
    }

    // ---- Dukascopy 数据源 ----

    /// 造一个 dukascopy 目录树：`{dir}/dukascopy/{symbol}/{yyyy}/{MM}/{dd}/*.bi5`，
    /// 其中月份 0-based（`00` = 1 月）。
    fn write_dukascopy_tree(dir: &Path, symbol: &str) {
        let d = dir
            .join("dukascopy")
            .join(symbol)
            .join("2025")
            .join("00")
            .join("02");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("BID_candles_min_1.bi5"), b"stub").unwrap();
    }

    #[test]
    fn test_dukascopy_lists_pairs() {
        let dir = TempDir::new().unwrap();
        write_dukascopy_tree(dir.path(), "EURUSD");
        write_dukascopy_tree(dir.path(), "XAUUSD");
        // 只有空目录的品种不能对外暴露（选了就是空图）。
        std::fs::create_dir_all(dir.path().join("dukascopy").join("USDJPY")).unwrap();

        let loader = DataLoader::new(dir.path());
        let pairs = loader.list_pairs(MarketType::Dukascopy).unwrap();
        assert_eq!(pairs, vec!["EURUSD", "XAUUSD"]);
    }

    #[test]
    fn test_dukascopy_lists_only_timeframes_with_files() {
        let dir = TempDir::new().unwrap();
        // write_dukascopy_tree 只落 M1 文件：没下 H1/D1 就不能对外宣称支持。
        write_dukascopy_tree(dir.path(), "EURUSD");

        let loader = DataLoader::new(dir.path());
        let tfs = loader
            .list_timeframes(MarketType::Dukascopy, "EURUSD")
            .unwrap();
        assert_eq!(tfs, vec!["M1"]);

        // 补上 H1/D1（含 ASK 侧）后按周期大小返回全部三个。
        let day_dir = dir
            .path()
            .join("dukascopy")
            .join("EURUSD")
            .join("2025")
            .join("00")
            .join("02");
        std::fs::write(day_dir.join("BID_candles_hour_1.bi5"), b"stub").unwrap();
        std::fs::write(day_dir.join("ASK_candles_day_1.bi5"), b"stub").unwrap();
        let tfs = loader
            .list_timeframes(MarketType::Dukascopy, "EURUSD")
            .unwrap();
        assert_eq!(tfs, vec!["M1", "H1", "D1"]);
    }

    #[test]
    fn test_dukascopy_rejects_unsupported_timeframe() {
        let dir = TempDir::new().unwrap();
        write_dukascopy_tree(dir.path(), "EURUSD");

        let loader = DataLoader::new(dir.path());
        let err = loader
            .load_klines_filtered(MarketType::Dukascopy, "EURUSD", "3m", None, None, None)
            .unwrap_err();
        // 周期不合法是**用户错误**（400），不是 500。
        assert!(matches!(err, DataError::InvalidParam(_)));
    }

    #[test]
    fn test_spot_is_not_advertised() {
        // Spot 变体仍可解析，但本地无数据源 ⇒ 不进 SUPPORTED 列表。
        assert!(!MarketType::SUPPORTED.contains(&MarketType::Spot));
        assert!(MarketType::SUPPORTED.contains(&MarketType::Futures));
        assert!(MarketType::SUPPORTED.contains(&MarketType::Dukascopy));
    }

    #[test]
    fn test_market_type_dukascopy_display_and_parse() {
        assert_eq!(MarketType::Dukascopy.to_string(), "dukascopy");
        assert_eq!(
            "Dukascopy".parse::<MarketType>().unwrap(),
            MarketType::Dukascopy
        );
    }
}
