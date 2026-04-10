use async_trait::async_trait;

use crate::{ExecOptions, Provider, ProviderConfig, ProviderError, Session};

pub struct CodexProvider {
    #[allow(dead_code)]
    config: ProviderConfig,
}

impl CodexProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Provider for CodexProvider {
    async fn execute(&self, _prompt: &str, _opts: ExecOptions) -> Result<Session, ProviderError> {
        unimplemented!("codex provider not yet implemented")
    }
}

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
        unimplemented!("cursor provider not yet implemented")
    }
}

pub struct OpencodeProvider {
    #[allow(dead_code)]
    config: ProviderConfig,
}

impl OpencodeProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Provider for OpencodeProvider {
    async fn execute(&self, _prompt: &str, _opts: ExecOptions) -> Result<Session, ProviderError> {
        unimplemented!("opencode provider not yet implemented")
    }
}
