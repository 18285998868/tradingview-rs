//! TradingView UDF（Universal Data Feed）协议适配。
//!
//! 端点挂在 `/udf/*`，与现有 `/api/*` 互不影响。UDF 的时间戳是**秒**，
//! 本地 K 线是**毫秒**；resolution 也需要映射到各数据源的实际周期。
//!
//! # 模块划分
//!
//! - `symbol`：symbol 解析与交易所/品种元数据
//! - `resolution`：UDF resolution 与本地周期的双向映射
//! - `history`：`from` / `to` / `countback` 到数据层过滤参数的转换
//! - `price`：从样本价格推导 `pricescale`
//! - `protocol`：UDF 响应结构（字段名即 JSON 契约）
//! - `handlers`：axum handler，只做参数解析与响应映射
//!
//! # 时间语义
//!
//! UDF 请求里的 `from` / `to` 是**秒**；传给数据层前统一转成毫秒，
//! 并按数据层「开区间」语义各偏移 1 毫秒。

mod handlers;
mod history;
mod price;
mod protocol;
mod resolution;
mod symbol;

pub(crate) use handlers::{
    config_handler, history_handler, search_handler, symbols_handler, time_handler,
};
