//! 服务配置：固定路径 TOML → 强类型配置 → 语义校验。

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::AppError;

/// 相对 crate 根的配置文件路径。
const CONFIG_REL_PATH: &str = "config/config.toml";

/// 配置文件路径覆盖环境变量。
pub const CONFIG_PATH_ENV: &str = "TRADINGVIEW_CONFIG";

/// 服务配置。
///
/// `#[serde(default)]` 让缺失字段回落到 [`Config::default`]，
/// `deny_unknown_fields` 让拼错的键直接报错而不是被静默忽略。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// 监听主机名或 IP。
    pub host: String,
    /// 监听端口。
    pub port: u16,
    /// 行情数据根目录（reader 按数据源自动追加子路径）。
    pub data_dir: PathBuf,
    /// 本地状态库路径；缺省为 `<data_dir>/tradingview.db`。
    ///
    /// SQLite 只存布局、绘图和偏好，不存行情。
    pub db_path: Option<PathBuf>,
    /// 前端静态资源目录；缺失时服务降级为纯 API 模式。
    pub frontend_dir: PathBuf,
    /// 允许的跨域来源（空 = 全放行，仅本地开发）。
    ///
    /// 非空时只放行列表内的 origin，并回显匹配的 origin。
    #[serde(default)]
    pub cors_origins: Vec<String>,
    /// 回测与 Python worker 配置。
    #[serde(default)]
    pub backtest: BacktestConfig,
}

/// 回测子系统配置。
///
/// `python_bin` 必须指向装有 vectorbt 1.2.0 的解释器；`worker_dir` 是包含
/// `backtest_worker/` 的目录。`output_dir` 缺省为 `<data_dir>/backtests`。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BacktestConfig {
    /// 是否启用回测 API 与后台 worker。
    pub enabled: bool,
    /// Python 解释器路径或命令名。
    pub python_bin: PathBuf,
    /// Python worker 根目录（包含 `backtest_worker/`）。
    pub worker_dir: PathBuf,
    /// 请求/结果/日志目录；缺省 `<data_dir>/backtests`。
    pub output_dir: Option<PathBuf>,
    /// 单次 worker 超时（秒）。
    pub timeout_secs: u64,
    /// 同时执行的 worker 进程上限。
    pub max_concurrent: usize,
    /// 对比运行数量上限（规格上限 10）。
    pub max_compare_runs: usize,
    /// 参数扫描展开后的运行数量上限。
    pub max_grid_runs: usize,
    /// 每条权益/水下曲线保留的最大点数。
    pub max_curve_points: usize,
    /// 单次运行保留的交易明细上限。
    pub max_trades: usize,
    /// 是否保存降采样权益曲线；关闭后仍保存水下曲线与指标。
    pub keep_equity: bool,
}

impl Default for BacktestConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            python_bin: PathBuf::from("python3"),
            worker_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("python"),
            output_dir: None,
            timeout_secs: 1800,
            max_concurrent: 2,
            max_compare_runs: 10,
            max_grid_runs: 100,
            max_curve_points: 2000,
            max_trades: 200_000,
            keep_equity: true,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 3001,
            data_dir: PathBuf::new(),
            db_path: None,
            frontend_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("frontend/dist"),
            cors_origins: Vec::new(),
            backtest: BacktestConfig::default(),
        }
    }
}

impl Config {
    /// 从配置文件读取配置。
    ///
    /// 路径优先级（见 `default_config_path`）。
    ///
    /// # Errors
    /// - 文件不存在或不可读 → [`AppError::ConfigRead`]
    /// - TOML 语法/字段非法 → [`AppError::ConfigParse`]
    /// - `data_dir` 为空 → [`AppError::Config`]
    pub fn load() -> Result<Self, AppError> {
        Self::load_from(&default_config_path())
    }

    /// 从指定路径读取配置。
    ///
    /// # Errors
    /// 同 [`Config::load`]。
    pub fn load_from(path: &Path) -> Result<Self, AppError> {
        let content = std::fs::read_to_string(path).map_err(|source| AppError::ConfigRead {
            path: path.to_path_buf(),
            source,
        })?;
        let cfg: Self = toml::from_str(&content).map_err(|source| AppError::ConfigParse {
            path: path.to_path_buf(),
            source,
        })?;
        cfg.validate()
    }

    /// 语义校验：`data_dir` 必填；`db_path` 若显式给出则不能为空。
    ///
    /// # Errors
    /// `data_dir` 或 `db_path` 为空、回测参数越界时返回 [`AppError::Config`]。
    pub fn validate(self) -> Result<Self, AppError> {
        if self.data_dir.as_os_str().is_empty() {
            return Err(AppError::Config("data_dir must not be empty".into()));
        }
        if self
            .db_path
            .as_ref()
            .is_some_and(|p| p.as_os_str().is_empty())
        {
            return Err(AppError::Config("db_path must not be empty".into()));
        }
        self.backtest.validate()?;
        Ok(self)
    }
}

impl BacktestConfig {
    /// 校验回测配置的语义边界。
    ///
    /// # Errors
    /// 路径为空或数值越界时返回 [`AppError::Config`]。
    pub fn validate(&self) -> Result<(), AppError> {
        if self.python_bin.as_os_str().is_empty() {
            return Err(AppError::Config(
                "backtest.python_bin must not be empty".into(),
            ));
        }
        if self.worker_dir.as_os_str().is_empty() {
            return Err(AppError::Config(
                "backtest.worker_dir must not be empty".into(),
            ));
        }
        if self
            .output_dir
            .as_ref()
            .is_some_and(|p| p.as_os_str().is_empty())
        {
            return Err(AppError::Config(
                "backtest.output_dir must not be empty".into(),
            ));
        }
        if self.timeout_secs == 0 {
            return Err(AppError::Config("backtest.timeout_secs must be > 0".into()));
        }
        if !(1..=16).contains(&self.max_concurrent) {
            return Err(AppError::Config(
                "backtest.max_concurrent must be in 1..=16".into(),
            ));
        }
        if !(2..=10).contains(&self.max_compare_runs) {
            return Err(AppError::Config(
                "backtest.max_compare_runs must be in 2..=10".into(),
            ));
        }
        if !(1..=1000).contains(&self.max_grid_runs) {
            return Err(AppError::Config(
                "backtest.max_grid_runs must be in 1..=1000".into(),
            ));
        }
        if !(10..=100_000).contains(&self.max_curve_points) {
            return Err(AppError::Config(
                "backtest.max_curve_points must be in 10..=100000".into(),
            ));
        }
        if !(1..=10_000_000).contains(&self.max_trades) {
            return Err(AppError::Config(
                "backtest.max_trades must be in 1..=10000000".into(),
            ));
        }
        Ok(())
    }
}

/// 配置文件路径优先级：
///
/// 1. `TRADINGVIEW_CONFIG` 环境变量（显式覆盖）；
/// 2. `./config/config.toml`（相对**当前工作目录** —— 部署时把配置放在
///    二进制旁边即可，不依赖构建机路径）；
/// 3. `<crate 根>/config/config.toml`（编译期绝对路径，只在开发机上有效）。
fn default_config_path() -> PathBuf {
    if let Some(p) = std::env::var_os(CONFIG_PATH_ENV).filter(|v| !v.is_empty()) {
        return PathBuf::from(p);
    }
    let cwd_rel = PathBuf::from(CONFIG_REL_PATH);
    if cwd_rel.exists() {
        return cwd_rel;
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join(CONFIG_REL_PATH)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_cfg(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("config.toml");
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn defaults_fill_missing_fields() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = write_cfg(dir.path(), "data_dir = \"/tmp/data\"\n");
        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg.host, "127.0.0.1");
        assert_eq!(cfg.port, 3001);
        assert_eq!(cfg.data_dir, PathBuf::from("/tmp/data"));
        assert!(cfg.frontend_dir.ends_with("frontend/dist"));
        assert!(cfg.db_path.is_none());
        assert!(cfg.backtest.enabled);
        assert_eq!(cfg.backtest.max_compare_runs, 10);
        assert_eq!(cfg.backtest.max_grid_runs, 100);
    }

    #[test]
    fn accepts_backtest_table() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = write_cfg(
            dir.path(),
            "data_dir = \"/tmp/data\"\n\
             [backtest]\n\
             enabled = false\n\
             python_bin = \"/opt/venv/bin/python\"\n\
             max_compare_runs = 4\n",
        );
        let cfg = Config::load_from(&path).unwrap();
        assert!(!cfg.backtest.enabled);
        assert_eq!(
            cfg.backtest.python_bin,
            PathBuf::from("/opt/venv/bin/python")
        );
        assert_eq!(cfg.backtest.max_compare_runs, 4);
    }

    #[test]
    fn rejects_backtest_compare_limit_above_ten() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = write_cfg(
            dir.path(),
            "data_dir = \"/tmp/data\"\n[backtest]\nmax_compare_runs = 11\n",
        );
        let err = Config::load_from(&path).unwrap_err();
        assert!(matches!(err, AppError::Config(_)));
        assert!(err.to_string().contains("max_compare_runs"));
    }

    #[test]
    fn accepts_db_path_override() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = write_cfg(
            dir.path(),
            "data_dir = \"/tmp/data\"\ndb_path = \"/tmp/state.db\"\n",
        );
        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg.db_path, Some(PathBuf::from("/tmp/state.db")));
    }

    #[test]
    fn rejects_empty_db_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = write_cfg(dir.path(), "data_dir = \"/tmp/data\"\ndb_path = \"\"\n");
        let err = Config::load_from(&path).unwrap_err();
        assert!(matches!(err, AppError::Config(_)));
        assert_eq!(err.to_string(), "config error: db_path must not be empty");
    }

    #[test]
    fn rejects_empty_data_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = write_cfg(dir.path(), "data_dir = \"\"\n");
        let err = Config::load_from(&path).unwrap_err();
        assert!(matches!(err, AppError::Config(_)));
        assert_eq!(err.to_string(), "config error: data_dir must not be empty");
    }

    #[test]
    fn rejects_unknown_field() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = write_cfg(dir.path(), "data_dir = \"/tmp/data\"\ntypo = 1\n");
        let err = Config::load_from(&path).unwrap_err();
        assert!(matches!(err, AppError::ConfigParse { .. }));
    }

    #[test]
    fn missing_file_reports_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("nope.toml");
        let err = Config::load_from(&path).unwrap_err();
        assert!(matches!(err, AppError::ConfigRead { .. }));
        // source 链保留底层 io 错误
        assert!(std::error::Error::source(&err).is_some());
    }
}
