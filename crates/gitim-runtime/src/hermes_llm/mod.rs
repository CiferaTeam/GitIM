//! Hermes-internal LLM provider × model selection layer.
//!
//! This module owns the static registry of built-in LLM providers that the
//! hermes runtime understands, plus the runtime introspection logic that
//! maps available API keys to live provider/model lists.

mod introspect;
mod models;
mod registry;

pub use introspect::{list_providers, LlmProvider, ProviderKind};
pub use models::{fetch_models, ModelEntry, ModelListResult};
pub use registry::{ApiProtocol, BuiltinProvider, BUILTIN_PROVIDERS};
