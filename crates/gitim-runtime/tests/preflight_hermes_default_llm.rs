#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Tests for `read_default_profile_llm` / `read_default_profile_llm_from`.
//!
//! These exercise the helper that resolves the `(provider, model)` pair from
//! a hermes profile's `config.yaml`. Used at add-time when an agent request
//! omits both `llm_provider` and `llm_model` — we read the user's default
//! profile and forward those values to chat-mode preflight.
//!
//! The path-based helper [`read_default_profile_llm_from`] is the workhorse:
//! tests target it directly so they don't mutate the process-global
//! `HERMES_HOME` env var (which races under cargo's multi-threaded runner).
//! One smoke test still touches `HERMES_HOME` to verify the env-aware
//! [`read_default_profile_llm`] wrapper, gated behind `#[serial]`.

use gitim_runtime::preflight::{read_default_profile_llm, read_default_profile_llm_from};
use tempfile::TempDir;

#[test]
fn returns_none_when_hermes_home_missing() {
    // Tempdir exists but contains no config.yaml at all.
    let tmp = TempDir::new().unwrap();
    let result = read_default_profile_llm_from(tmp.path());
    assert!(
        result.is_none(),
        "expected None when config.yaml is absent, got {result:?}"
    );
}

#[test]
fn returns_none_when_config_yaml_lacks_model_default() {
    let tmp = TempDir::new().unwrap();
    let yaml = "model:\n  provider: anthropic\n";
    std::fs::write(tmp.path().join("config.yaml"), yaml).unwrap();
    let result = read_default_profile_llm_from(tmp.path());
    assert!(
        result.is_none(),
        "expected None when model.default missing, got {result:?}"
    );
}

#[test]
fn returns_none_when_config_yaml_lacks_model_provider() {
    let tmp = TempDir::new().unwrap();
    let yaml = "model:\n  default: claude-opus-4-7\n";
    std::fs::write(tmp.path().join("config.yaml"), yaml).unwrap();
    let result = read_default_profile_llm_from(tmp.path());
    assert!(
        result.is_none(),
        "expected None when model.provider missing, got {result:?}"
    );
}

#[test]
fn returns_some_when_both_present() {
    let tmp = TempDir::new().unwrap();
    let yaml = "model:\n  default: claude-opus-4-7\n  provider: anthropic\n";
    std::fs::write(tmp.path().join("config.yaml"), yaml).unwrap();
    let result = read_default_profile_llm_from(tmp.path());
    assert_eq!(
        result,
        Some(("anthropic".to_string(), "claude-opus-4-7".to_string()))
    );
}

#[test]
fn returns_none_when_config_yaml_malformed() {
    let tmp = TempDir::new().unwrap();
    // Not valid YAML (unclosed brace, garbled mapping syntax).
    let yaml = "model: { provider: anthropic, default: oops\n  - bad\n: not yaml :";
    std::fs::write(tmp.path().join("config.yaml"), yaml).unwrap();
    let result = read_default_profile_llm_from(tmp.path());
    assert!(
        result.is_none(),
        "expected None for malformed YAML, got {result:?}"
    );
}

#[test]
fn returns_none_when_model_is_scalar() {
    // Defensive: `model:` as a string rather than a map. Hermes wouldn't
    // emit this, but a hand-edited config might. Should not panic.
    let tmp = TempDir::new().unwrap();
    let yaml = "model: claude-opus-4-7\n";
    std::fs::write(tmp.path().join("config.yaml"), yaml).unwrap();
    let result = read_default_profile_llm_from(tmp.path());
    assert!(result.is_none());
}

#[test]
fn returns_none_when_model_provider_is_non_string() {
    // `model.provider: 42` — yaml lets us put a number there. We require a
    // string, so this should degrade to None rather than coerce.
    let tmp = TempDir::new().unwrap();
    let yaml = "model:\n  provider: 42\n  default: claude-opus-4-7\n";
    std::fs::write(tmp.path().join("config.yaml"), yaml).unwrap();
    let result = read_default_profile_llm_from(tmp.path());
    assert!(result.is_none());
}

// ─── env-aware wrapper smoke test ────────────────────────────────────────────
//
// `read_default_profile_llm` reads HERMES_HOME, a process-global env var.
// Serial the whole module to avoid races with hermes_profile.rs tests that
// also touch HERMES_HOME.

#[test]
#[serial_test::serial(hermes_home_env)]
fn env_wrapper_respects_hermes_home() {
    let tmp = TempDir::new().unwrap();
    let yaml = "model:\n  default: claude-opus-4-7\n  provider: anthropic\n";
    std::fs::write(tmp.path().join("config.yaml"), yaml).unwrap();

    // Snapshot prior HERMES_HOME so we don't clobber a developer's local env.
    let prior = std::env::var_os("HERMES_HOME");
    std::env::set_var("HERMES_HOME", tmp.path());

    let result = read_default_profile_llm();

    match prior {
        Some(v) => std::env::set_var("HERMES_HOME", v),
        None => std::env::remove_var("HERMES_HOME"),
    }

    assert_eq!(
        result,
        Some(("anthropic".to_string(), "claude-opus-4-7".to_string()))
    );
}
