//! HTTP gateway for the Aegis sandbox.
//!
//! Exposes the forward proxy and header rewriting modules so both the binary
//! and the integration tests share the same `serve` implementation.

pub mod proxy;
pub mod rewrite;
