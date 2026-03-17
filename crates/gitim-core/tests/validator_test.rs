use gitim_core::validator::{validate_user_meta, validate_channel_meta, validate_config, validate_channel_name};

#[test]
fn test_valid_user_meta() {
    let json = r#"{"display_name":"Nexus","role":"ceo","introduction":"hello"}"#;
    assert!(validate_user_meta(json).is_ok());
}

#[test]
fn test_user_meta_missing_field() {
    let json = r#"{"display_name":"Nexus","role":"ceo"}"#;
    assert!(validate_user_meta(json).is_err());
}

#[test]
fn test_user_meta_display_name_too_long() {
    let name = "x".repeat(65);
    let json = format!(r#"{{"display_name":"{}","role":"ceo","introduction":"hi"}}"#, name);
    assert!(validate_user_meta(&json).is_err());
}

#[test]
fn test_valid_channel_meta() {
    let json = r#"{"display_name":"General","created_by":"nexus","created_at":"20250316T120000Z","introduction":"hello"}"#;
    assert!(validate_channel_meta(json).is_ok());
}

#[test]
fn test_channel_meta_missing_field() {
    let json = r#"{"display_name":"General","created_by":"nexus"}"#;
    assert!(validate_channel_meta(json).is_err());
}

#[test]
fn test_channel_meta_invalid_created_at() {
    let json = r#"{"display_name":"General","created_by":"nexus","created_at":"not-a-date","introduction":"hello"}"#;
    assert!(validate_channel_meta(json).is_err());
}

#[test]
fn test_channel_meta_invalid_created_by() {
    let json = r#"{"display_name":"General","created_by":"INVALID","created_at":"20250316T120000Z","introduction":"hello"}"#;
    assert!(validate_channel_meta(json).is_err());
}

#[test]
fn test_valid_channel_names() {
    assert!(validate_channel_name("general").is_ok());
    assert!(validate_channel_name("dev").is_ok());
    assert!(validate_channel_name("project-alpha").is_ok());
    assert!(validate_channel_name("a-b-c").is_ok());
    assert!(validate_channel_name("team2").is_ok());
}

#[test]
fn test_invalid_channel_names() {
    assert!(validate_channel_name("").is_err());
    assert!(validate_channel_name("-general").is_err());
    assert!(validate_channel_name("general-").is_err());
    assert!(validate_channel_name("gen--eral").is_err());
    assert!(validate_channel_name("General").is_err());
    assert!(validate_channel_name("gen eral").is_err());
    let long = "a".repeat(33);
    assert!(validate_channel_name(&long).is_err());
}

#[test]
fn test_valid_config() {
    assert!(validate_config("version: 1").is_ok());
    assert!(validate_config("version: 1\ndaemon:\n  sync_interval: 60").is_ok());
}

#[test]
fn test_invalid_config_version() {
    assert!(validate_config("version: 2").is_err());
}

#[test]
fn test_config_missing_version() {
    assert!(validate_config("daemon:\n  sync_interval: 30").is_err());
}

#[test]
fn test_config_with_endpoint() {
    let yaml = "version: 1\nendpoint: github\n";
    let config = validate_config(yaml).unwrap();
    assert_eq!(config.endpoint, "github");
    assert_eq!(config.endpoint_url, "");
}

#[test]
fn test_config_with_gitea_endpoint() {
    let yaml = "version: 1\nendpoint: gitea\nendpoint_url: https://gitea.example.com\n";
    let config = validate_config(yaml).unwrap();
    assert_eq!(config.endpoint, "gitea");
    assert_eq!(config.endpoint_url, "https://gitea.example.com");
}

#[test]
fn test_config_endpoint_defaults() {
    let yaml = "version: 1\n";
    let config = validate_config(yaml).unwrap();
    assert_eq!(config.endpoint, "github");
    assert_eq!(config.endpoint_url, "");
}
