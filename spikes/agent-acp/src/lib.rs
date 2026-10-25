#![allow(clippy::collapsible_if)]

pub mod adapter;
pub mod model;
pub mod normalizer;
pub mod opencode_config;
pub mod sanitize;
pub mod selfcheck;

pub use adapter::{AcpAdapter, ClientCapabilityConfig};
pub use normalizer::AcpNormalizer;
