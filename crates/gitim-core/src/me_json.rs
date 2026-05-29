//! Typed schema for `<repo>/.gitim/me.json`.
//!
//! Source of truth for fields on disk. Both daemon (write) and runtime/CLI
//! (read) go through this. Unknown fields are preserved via `extra` so older
//! tools and future fields don't get silently dropped on rewrite.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeJson {
    /// Handler (GitHub handle, lowercased). `None` for guest mode (stored as
    /// `null` on disk to keep the field present).
    #[serde(default)]
    pub handler: Option<String>,

    /// Human-readable display name, written at onboard time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,

    /// Auth provider hint: "github" / "git" / "gitlab" / "gitea" / ...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_server: Option<String>,

    /// ISO compact timestamp of onboard (e.g. `20260506T120000Z`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub onboarded_at: Option<String>,

    /// Verified GitHub email (github mode only). Source of truth for daemon
    /// commit author email. Preserved across re-onboard via merge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_email: Option<String>,

    /// Guest mode flag. Mutually exclusive with a real handler — `clear_guest`
    /// removes it when a guest later claims an identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guest: Option<bool>,

    /// Admin-mode flag for the human daemon. This is persisted so a daemon-only
    /// restart can restore the WebUI's workspace-wide visibility without a
    /// fresh onboard call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin: Option<bool>,

    // -- Agent runtime config (per-clone, not git-synced; written by
    // gitim-runtime `add_agent`. Daemon does not read these.) --
    /// Agent provider name: `claude` / `hermes` / `gemini` / `opencode` / ...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,

    /// Provider model id (e.g. `sonnet-4-6`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Effort level for the `claude` provider (`low` / `medium` / `high` /
    /// `xhigh` / `max`). Passed to the CLI as `--effort`. `None` for other
    /// providers or when unset. Preserved across re-onboard via merge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,

    /// Custom system prompt for the agent loop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,

    /// Extra env vars merged into the provider process env. `BTreeMap` for
    /// deterministic on-disk key order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,

    /// Hermes-internal LLM provider id (e.g. `minimax-cn`, `custom:my-glm`).
    /// Only meaningful when `provider == "hermes"`. None for other providers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_provider: Option<String>,

    /// Hermes-internal LLM model id (e.g. `MiniMax-M2.7-highspeed`).
    /// Only meaningful when `provider == "hermes"`. None for other providers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_model: Option<String>,

    /// Forward-compat: any field this version doesn't know about is captured
    /// here and round-tripped on rewrite. Both daemon and runtime currently
    /// rewrite me.json; without this, future fields and user annotations
    /// would silently disappear.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl MeJson {
    /// Layer `patch` on top of `self`. Each `Some` in `patch` overrides;
    /// each `None` preserves the existing value. This is the canonical
    /// "re-onboard without erasing other fields" operation.
    pub fn merged_with(mut self, patch: MeJson) -> Self {
        if patch.handler.is_some() {
            self.handler = patch.handler;
        }
        if patch.display_name.is_some() {
            self.display_name = patch.display_name;
        }
        if patch.git_server.is_some() {
            self.git_server = patch.git_server;
        }
        if patch.onboarded_at.is_some() {
            self.onboarded_at = patch.onboarded_at;
        }
        if patch.github_email.is_some() {
            self.github_email = patch.github_email;
        }
        if patch.guest.is_some() {
            self.guest = patch.guest;
        }
        if patch.admin.is_some() {
            self.admin = patch.admin;
        }
        if patch.provider.is_some() {
            self.provider = patch.provider;
        }
        if patch.model.is_some() {
            self.model = patch.model;
        }
        if patch.effort.is_some() {
            self.effort = patch.effort;
        }
        if patch.system_prompt.is_some() {
            self.system_prompt = patch.system_prompt;
        }
        if patch.env.is_some() {
            self.env = patch.env;
        }
        if patch.llm_provider.is_some() {
            self.llm_provider = patch.llm_provider;
        }
        if patch.llm_model.is_some() {
            self.llm_model = patch.llm_model;
        }
        self.extra.extend(patch.extra);
        self
    }

    /// Explicitly drop the `guest` flag. Used when a guest later claims an
    /// identity; `merged_with` alone cannot express "remove" because `None`
    /// in a patch means "preserve".
    pub fn clear_guest(&mut self) {
        self.guest = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    /// Wire shape produced by daemon `write_me_json` in github mode.
    #[test]
    fn deserialize_full_identity_object() {
        let raw = r#"{
            "handler": "alice",
            "display_name": "Alice W",
            "git_server": "github",
            "onboarded_at": "20260506T120000Z",
            "github_email": "alice@example.com",
            "admin": true
        }"#;
        let me: MeJson = serde_json::from_str(raw).unwrap();
        assert_eq!(me.handler.as_deref(), Some("alice"));
        assert_eq!(me.display_name.as_deref(), Some("Alice W"));
        assert_eq!(me.git_server.as_deref(), Some("github"));
        assert_eq!(me.onboarded_at.as_deref(), Some("20260506T120000Z"));
        assert_eq!(me.github_email.as_deref(), Some("alice@example.com"));
        assert_eq!(me.guest, None);
        assert_eq!(me.admin, Some(true));
    }

    /// Wire shape produced by daemon `write_guest_me_json`. `handler` is
    /// explicitly null so the field is present on disk.
    #[test]
    fn deserialize_guest_object() {
        let raw = r#"{
            "handler": null,
            "guest": true,
            "onboarded_at": "20260506T120000Z"
        }"#;
        let me: MeJson = serde_json::from_str(raw).unwrap();
        assert_eq!(me.handler, None);
        assert_eq!(me.guest, Some(true));
        assert_eq!(me.onboarded_at.as_deref(), Some("20260506T120000Z"));
    }

    /// Wire shape after runtime `add_agent` writes provider/model/etc. on top
    /// of an existing identity. Per-clone, not git-synced (CLAUDE.md).
    #[test]
    fn deserialize_runtime_config_object() {
        let raw = r#"{
            "handler": "alice",
            "display_name": "Alice W",
            "provider": "claude",
            "model": "sonnet-4-6",
            "system_prompt": "be helpful",
            "env": {"FOO": "bar", "BAZ": "qux"}
        }"#;
        let me: MeJson = serde_json::from_str(raw).unwrap();
        assert_eq!(me.provider.as_deref(), Some("claude"));
        assert_eq!(me.model.as_deref(), Some("sonnet-4-6"));
        assert_eq!(me.system_prompt.as_deref(), Some("be helpful"));
        let env = me.env.as_ref().unwrap();
        assert_eq!(env.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(env.get("BAZ").map(String::as_str), Some("qux"));
    }

    /// `handler` is the only field that must stay present on disk even when
    /// None — guest mode writes it as `null`. All other None fields are
    /// omitted to keep me.json small and intentional.
    #[test]
    fn serialize_guest_keeps_null_handler() {
        let me = MeJson {
            handler: None,
            guest: Some(true),
            onboarded_at: Some("20260506T120000Z".into()),
            ..Default::default()
        };
        let v: serde_json::Value = serde_json::to_value(&me).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.get("handler"), Some(&serde_json::Value::Null));
        assert_eq!(obj.get("guest"), Some(&serde_json::Value::Bool(true)));
        assert!(!obj.contains_key("display_name"));
        assert!(!obj.contains_key("github_email"));
        assert!(!obj.contains_key("provider"));
    }

    /// `merged_with` is how callers express "patch these fields on top of
    /// existing me.json". This is the typed replacement for the ad-hoc
    /// `read_me_json_object` + `obj.insert(...)` dance in onboard.rs.
    #[test]
    fn merge_other_some_overrides_self() {
        let base = MeJson {
            handler: Some("alice".into()),
            display_name: Some("old name".into()),
            github_email: Some("old@example.com".into()),
            admin: Some(false),
            ..Default::default()
        };
        let patch = MeJson {
            display_name: Some("new name".into()),
            github_email: Some("new@example.com".into()),
            admin: Some(true),
            ..Default::default()
        };
        let merged = base.merged_with(patch);
        assert_eq!(merged.handler.as_deref(), Some("alice"));
        assert_eq!(merged.display_name.as_deref(), Some("new name"));
        assert_eq!(merged.github_email.as_deref(), Some("new@example.com"));
        assert_eq!(merged.admin, Some(true));
    }

    /// CLAUDE.md merge semantics: re-onboard without `github_email` must keep
    /// the existing value, not blank it out. Tested for every Option field.
    #[test]
    fn merge_other_none_preserves_self() {
        let base = MeJson {
            handler: Some("alice".into()),
            display_name: Some("Alice W".into()),
            git_server: Some("github".into()),
            github_email: Some("alice@example.com".into()),
            admin: Some(true),
            provider: Some("claude".into()),
            model: Some("sonnet-4-6".into()),
            ..Default::default()
        };
        let empty_patch = MeJson::default();
        let merged = base.clone().merged_with(empty_patch);
        assert_eq!(merged, base);
    }

    /// `clear_guest` drops the `guest` field on disk via `None` +
    /// `skip_serializing_if` — mirrors the on-disk shape after a guest
    /// claims a real identity.
    #[test]
    fn clear_guest_removes_field_from_disk() {
        let mut me = MeJson {
            handler: None,
            guest: Some(true),
            ..Default::default()
        };
        me.clear_guest();
        let v: serde_json::Value = serde_json::to_value(&me).unwrap();
        let obj = v.as_object().unwrap();
        assert!(!obj.contains_key("guest"));
    }

    /// Forward-compat: a future daemon writes `daemon_version` and the user
    /// hand-edits `note` into me.json. An older runtime reading + merging +
    /// re-writing must not silently drop these fields.
    #[test]
    fn unknown_fields_round_trip_via_extra() {
        let raw = r#"{
            "handler": "alice",
            "daemon_version": "9.9.9",
            "note": "remember to rotate the key in june"
        }"#;
        let me: MeJson = serde_json::from_str(raw).unwrap();
        assert_eq!(me.handler.as_deref(), Some("alice"));
        assert_eq!(
            me.extra.get("daemon_version").and_then(|v| v.as_str()),
            Some("9.9.9"),
        );
        assert_eq!(
            me.extra.get("note").and_then(|v| v.as_str()),
            Some("remember to rotate the key in june"),
        );

        let v: serde_json::Value = serde_json::to_value(&me).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(
            obj.get("daemon_version").and_then(|v| v.as_str()),
            Some("9.9.9")
        );
        assert_eq!(
            obj.get("note").and_then(|v| v.as_str()),
            Some("remember to rotate the key in june"),
        );
    }

    /// Merge must combine extras the same way: `patch` keys override, others
    /// on `self` are preserved.
    #[test]
    fn merge_combines_extra_fields() {
        let base = serde_json::from_str::<MeJson>(
            r#"{"handler":"alice","old_field":"keep","shared":"old"}"#,
        )
        .unwrap();
        let patch =
            serde_json::from_str::<MeJson>(r#"{"new_field":"new","shared":"new"}"#).unwrap();
        let merged = base.merged_with(patch);
        assert_eq!(
            merged.extra.get("old_field").and_then(|v| v.as_str()),
            Some("keep")
        );
        assert_eq!(
            merged.extra.get("new_field").and_then(|v| v.as_str()),
            Some("new")
        );
        assert_eq!(
            merged.extra.get("shared").and_then(|v| v.as_str()),
            Some("new")
        );
    }

    #[test]
    fn serialize_omits_none_non_handler_fields() {
        let me = MeJson {
            handler: Some("alice".into()),
            display_name: Some("Alice W".into()),
            ..Default::default()
        };
        let v: serde_json::Value = serde_json::to_value(&me).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 2);
        assert!(obj.contains_key("handler"));
        assert!(obj.contains_key("display_name"));
    }

    // --- llm_provider / llm_model field tests ---

    /// T1: Roundtrip: Both new fields survive serialize → deserialize unchanged.
    #[test]
    fn serde_roundtrip_includes_llm_fields() {
        let original = MeJson {
            handler: Some("bob".into()),
            provider: Some("hermes".into()),
            llm_provider: Some("minimax-cn".into()),
            llm_model: Some("MiniMax-M2.7-highspeed".into()),
            ..Default::default()
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: MeJson = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.llm_provider.as_deref(), Some("minimax-cn"));
        assert_eq!(decoded.llm_model.as_deref(), Some("MiniMax-M2.7-highspeed"));
    }

    /// T2: When None, the keys must be absent from the serialized JSON.
    #[test]
    fn serde_skip_serializing_when_none() {
        let me = MeJson {
            handler: Some("bob".into()),
            provider: Some("hermes".into()),
            ..Default::default()
        };
        let v: serde_json::Value = serde_json::to_value(&me).unwrap();
        let obj = v.as_object().unwrap();
        assert!(
            !obj.contains_key("llm_provider"),
            "llm_provider must be absent when None"
        );
        assert!(
            !obj.contains_key("llm_model"),
            "llm_model must be absent when None"
        );
    }

    /// T3: patch with Some llm_provider/llm_model overrides base.
    #[test]
    fn merged_with_overrides_llm_fields_when_some() {
        let base = MeJson {
            handler: Some("bob".into()),
            llm_provider: Some("old-provider".into()),
            llm_model: Some("old-model".into()),
            ..Default::default()
        };
        let patch = MeJson {
            llm_provider: Some("minimax-cn".into()),
            llm_model: Some("MiniMax-M2.7-highspeed".into()),
            ..Default::default()
        };
        let merged = base.merged_with(patch);
        assert_eq!(merged.llm_provider.as_deref(), Some("minimax-cn"));
        assert_eq!(merged.llm_model.as_deref(), Some("MiniMax-M2.7-highspeed"));
    }

    /// T4: patch with None llm_provider/llm_model preserves base values.
    #[test]
    fn merged_with_preserves_llm_fields_when_patch_none() {
        let base = MeJson {
            handler: Some("bob".into()),
            llm_provider: Some("minimax-cn".into()),
            llm_model: Some("MiniMax-M2.7-highspeed".into()),
            ..Default::default()
        };
        let patch = MeJson {
            handler: Some("bob".into()),
            ..Default::default() // llm_provider and llm_model are None
        };
        let merged = base.merged_with(patch);
        assert_eq!(merged.llm_provider.as_deref(), Some("minimax-cn"));
        assert_eq!(merged.llm_model.as_deref(), Some("MiniMax-M2.7-highspeed"));
    }

    /// T5: Forward-compat: the existing extra BTreeMap round-trip behavior is
    /// unaffected by our new fields. Exercises the existing test scenario.
    #[test]
    fn forward_compat_unknown_field_preserved() {
        let raw = r#"{
            "handler": "carol",
            "llm_provider": "minimax-cn",
            "future_flag": true,
            "custom_note": "rotate key in june"
        }"#;
        let me: MeJson = serde_json::from_str(raw).unwrap();
        // Known new field is recognized and NOT in extra
        assert_eq!(me.llm_provider.as_deref(), Some("minimax-cn"));
        assert!(
            !me.extra.contains_key("llm_provider"),
            "known field must not leak into extra"
        );
        // Truly unknown field lands in extra and round-trips
        assert_eq!(
            me.extra.get("custom_note").and_then(|v| v.as_str()),
            Some("rotate key in june"),
        );
        let v: serde_json::Value = serde_json::to_value(&me).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(
            obj.get("custom_note").and_then(|v| v.as_str()),
            Some("rotate key in june")
        );
        assert_eq!(
            obj.get("llm_provider").and_then(|v| v.as_str()),
            Some("minimax-cn")
        );
    }

    // --- effort field tests ---

    /// effort survives serialize → deserialize and is omitted when None.
    #[test]
    fn serde_effort_roundtrip_and_skip_when_none() {
        let with = MeJson {
            handler: Some("alice".into()),
            provider: Some("claude".into()),
            effort: Some("xhigh".into()),
            ..Default::default()
        };
        let decoded: MeJson = serde_json::from_str(&serde_json::to_string(&with).unwrap()).unwrap();
        assert_eq!(decoded.effort.as_deref(), Some("xhigh"));

        let without = MeJson {
            handler: Some("alice".into()),
            provider: Some("claude".into()),
            ..Default::default()
        };
        let obj = serde_json::to_value(&without).unwrap();
        assert!(!obj.as_object().unwrap().contains_key("effort"));
    }

    /// Merge mirrors model: Some overrides, None preserves (re-onboard safety).
    #[test]
    fn merged_with_effort_some_overrides_none_preserves() {
        let base = MeJson {
            handler: Some("alice".into()),
            effort: Some("high".into()),
            ..Default::default()
        };
        assert_eq!(
            base.clone()
                .merged_with(MeJson {
                    effort: Some("max".into()),
                    ..Default::default()
                })
                .effort
                .as_deref(),
            Some("max")
        );
        assert_eq!(
            base.merged_with(MeJson::default()).effort.as_deref(),
            Some("high")
        );
    }
}
