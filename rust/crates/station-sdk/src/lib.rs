//! SDK core ported from packages/sdk: common base, facade, and the leaf
//! consumers (storage, config, history, ipc, resources, session) as typed
//! traits and state over tokio sync primitives, plus the stream consumers
//! (search, activity) over tokio-stream. The webview-coupled consumers
//! (tabs, react) are trait surfaces only until the webview host lands.

pub mod activity;
pub mod common;
pub mod config;
pub mod facade;
pub mod history;
pub mod ipc;
pub mod react;
pub mod resources;
pub mod search;
pub mod session;
pub mod storage;
pub mod tabs;

pub use common::ConsumerId;
pub use facade::{Sdk, SdkOptions};
