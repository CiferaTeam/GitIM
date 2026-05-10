//! Hermes-internal LLM provider × model selection layer.
//!
//! This module owns the static registry of built-in LLM providers that the
//! hermes runtime understands, plus the runtime introspection logic that
//! maps available API keys to live provider/model lists.

mod registry;

pub use registry::{ApiProtocol, BuiltinProvider, BUILTIN_PROVIDERS};
