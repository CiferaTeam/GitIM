use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    pub version: u32,
    #[serde(default = "default_endpoint")]
    pub endpoint: String,
    #[serde(default)]
    pub endpoint_url: String,
    #[serde(default)]
    pub daemon: DaemonConfig,
}

fn default_endpoint() -> String {
    "github".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DaemonConfig {
    #[serde(default = "default_sync_interval")]
    pub sync_interval: u32,
    #[serde(default)]
    pub debug_http: bool,
    #[serde(default = "default_debug_port")]
    pub debug_port: u16,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            sync_interval: 3,
            debug_http: false,
            debug_port: 3000,
        }
    }
}

fn default_sync_interval() -> u32 { 3 }
fn default_debug_port() -> u16 { 3000 }

impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            endpoint: default_endpoint(),
            endpoint_url: String::new(),
            daemon: DaemonConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_default_values() {
        let c = Config::default();
        assert_eq!(c.version, 1);
        assert_eq!(c.endpoint, "github");
        assert_eq!(c.endpoint_url, "");
        assert_eq!(c.daemon.sync_interval, 3);
        assert!(!c.daemon.debug_http);
        assert_eq!(c.daemon.debug_port, 3000);
    }

    #[test]
    fn config_default_roundtrips_through_yaml() {
        let c = Config::default();
        let yaml = serde_yaml::to_string(&c).expect("serialize");
        let parsed: Config = serde_yaml::from_str(&yaml).expect("deserialize");
        assert_eq!(c, parsed);
    }

    #[test]
    fn config_default_passes_validate_config() {
        let c = Config::default();
        let yaml = serde_yaml::to_string(&c).expect("serialize");
        let validated = crate::validator::validate_config(&yaml).expect("validate");
        assert_eq!(validated, c);
    }
}
