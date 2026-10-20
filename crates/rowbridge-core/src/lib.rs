//! RowBridge 核心引擎。
//!
//! 设计要点见 `docs/DESIGN.md`。这个 crate 不依赖 Tauri，可以单独用 CLI 跑，
//! 便于测试与社区贡献。
//!
//! 模块划分：
//! - [`config`]  连接与任务配置
//! - [`introspect`]  information_schema 内省
//! - [`ddl`]      SHOW CREATE TABLE 改写
//! - [`preflight`] 预检
//! - [`engine`]   同步主循环
//! - [`value`]    值的无损搬运

pub mod config;
pub mod ddl;
pub mod engine;
pub mod error;
pub mod introspect;
pub mod preflight;
pub mod value;

pub use error::{Error, Result};
