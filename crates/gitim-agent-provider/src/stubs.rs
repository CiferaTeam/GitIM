use async_trait::async_trait;

use crate::{ExecOptions, Provider, ProviderConfig, ProviderError, Session};

pub struct CursorProvider {
    #[allow(dead_code)]
    config: ProviderConfig,
}

impl CursorProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Provider for CursorProvider {
    async fn execute(&self, _prompt: &str, _opts: ExecOptions) -> Result<Session, ProviderError> {
        Err(ProviderError::NotImplemented("cursor".to_string()))
    }
}
