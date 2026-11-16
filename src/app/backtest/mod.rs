//! 回测子系统：模型、持久化、任务服务、对比与 HTTP 路由。
//!
//! 计算在 Python vectorbt worker 中完成（见 `docs/adr/0001-rust-python-vectorbt.md`）；
//! 本模块负责请求校验、任务状态、结果落库、对比与导出。

pub(crate) mod compare;
pub mod model;
pub(crate) mod routes;
pub mod service;
pub(crate) mod store;
