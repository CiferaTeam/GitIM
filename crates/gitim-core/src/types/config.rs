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
            sync_interval: 30,
            debug_http: false,
            debug_port: 3000,
        }
    }
}

fn default_sync_interval() -> u32 { 30 }
fn default_debug_port() -> u16 { 3000 }
