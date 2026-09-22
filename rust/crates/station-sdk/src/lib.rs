//! SDK core ported from packages/sdk: common base, facade, and the leaf
//! consumers (storage, config, history, ipc, resources, session) as typed
//! traits and state over tokio sync primitives.
//!
//! Stream consumers (search, activity, tabs, react) live in follow-up slices.

pub mod common;
pub mod config;
pub mod facade;
pub mod history;
pub mod ipc;
pub mod resources;
pub mod session;
pub mod storage;

pub use common::ConsumerId;
pub use facade::{Sdk, SdkOptions};
