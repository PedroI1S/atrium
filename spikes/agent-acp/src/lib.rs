#![allow(clippy::collapsible_if)]

pub mod adapter;
pub mod normalizer;

pub use adapter::{AcpAdapter, ClientCapabilityConfig};
pub use normalizer::AcpNormalizer;
