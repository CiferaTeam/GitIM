# GitIM v1 Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the GitIM v1 protocol — a text-file + Git based IM system with Rust daemon and TypeScript CLI.

**Architecture:** Rust cargo workspace with three crates (`gitim-core`, `gitim-daemon`, `gitim-sync`) plus a TypeScript CLI package (`gitim-cli`). The daemon is the single binary that links all Rust crates. The CLI is a thin client that talks to the daemon over Unix socket.

**Tech Stack:** Rust (tokio, serde, regex, axum), TypeScript (Node.js, commander)

**Spec:** `docs/superpowers/specs/2026-03-16-gitim-v1-design.md`

---

## Dependency Graph

```
Phase 0: Scaffolding + Core Types (serial, must complete first)
  │
  ├→ Stream A: gitim-core (parser + validator)
  ├→ Stream B: gitim-daemon (server + lifecycle)
  ├→ Stream C: gitim-sync (git engine)
  └→ Stream D: gitim-cli (TypeScript CLI)
```

Stream A is the critical path — B and C depend on A's types and traits. D depends on B's API contract (can mock). All four streams can begin in parallel after Phase 0, since A's type definitions are established there.

---

## Chunk 1: Phase 0 — Scaffolding

### Task 0.1: Rust Workspace Setup

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `crates/gitim-core/Cargo.toml`
- Create: `crates/gitim-core/src/lib.rs`
- Create: `crates/gitim-daemon/Cargo.toml`
- Create: `crates/gitim-daemon/src/main.rs`
- Create: `crates/gitim-sync/Cargo.toml`
- Create: `crates/gitim-sync/src/lib.rs`

- [ ] **Step 1: Create workspace Cargo.toml**

```toml
[workspace]
members = ["crates/gitim-core", "crates/gitim-daemon", "crates/gitim-sync"]
resolver = "2"

[workspace.package]
version = "0.1.0"
edition = "2021"
license = "MIT"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
regex = "1"
thiserror = "2"
chrono = { version = "0.4", features = ["serde"] }
tracing = "0.1"
tracing-subscriber = "0.3"
```

- [ ] **Step 2: Create gitim-core crate**

`crates/gitim-core/Cargo.toml`:
```toml
[package]
name = "gitim-core"
version.workspace = true
edition.workspace = true

[dependencies]
serde.workspace = true
serde_json.workspace = true
regex.workspace = true
thiserror.workspace = true
chrono.workspace = true

[dev-dependencies]
pretty_assertions = "1"
```

`crates/gitim-core/src/lib.rs`:
```rust
pub mod types;
pub mod parser;
pub mod validator;
```

- [ ] **Step 3: Create gitim-daemon crate**

`crates/gitim-daemon/Cargo.toml`:
```toml
[package]
name = "gitim-daemon"
version.workspace = true
edition.workspace = true

[dependencies]
gitim-core = { path = "../gitim-core" }
gitim-sync = { path = "../gitim-sync" }
tokio.workspace = true
serde.workspace = true
serde_json.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
axum = "0.8"
tower = "0.5"
hyper = "1"
hyper-util = "0.1"
tokio-stream = "0.1"
```

`crates/gitim-daemon/src/main.rs`:
```rust
fn main() {
    println!("gitim-daemon");
}
```

- [ ] **Step 4: Create gitim-sync crate**

`crates/gitim-sync/Cargo.toml`:
```toml
[package]
name = "gitim-sync"
version.workspace = true
edition.workspace = true

[dependencies]
gitim-core = { path = "../gitim-core" }
tokio.workspace = true
thiserror.workspace = true
tracing.workspace = true
```

`crates/gitim-sync/src/lib.rs`:
```rust
pub mod git;
pub mod watcher;
```

- [ ] **Step 5: Verify workspace builds**

Run: `cargo check`
Expected: compiles with no errors

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/
git commit -m "chore: initialize rust workspace with three crates"
```

---

### Task 0.2: Core Type Definitions

**Files:**
- Create: `crates/gitim-core/src/types.rs`
- Create: `crates/gitim-core/src/types/message.rs`
- Create: `crates/gitim-core/src/types/meta.rs`
- Create: `crates/gitim-core/src/types/config.rs`
- Create: `crates/gitim-core/src/types/handler.rs`

- [ ] **Step 1: Create handler type**

`crates/gitim-core/src/types/handler.rs`:
```rust
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum HandlerError {
    #[error("handler is empty")]
    Empty,
    #[error("handler exceeds 39 characters")]
    TooLong,
    #[error("handler contains invalid character: {0}")]
    InvalidChar(char),
    #[error("handler must not start or end with hyphen")]
    HyphenBoundary,
    #[error("handler must not contain consecutive hyphens")]
    ConsecutiveHyphens,
    #[error("handler 'system' is reserved")]
    Reserved,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Handler(String);

impl Handler {
    pub fn new(s: &str) -> Result<Self, HandlerError> {
        if s.is_empty() {
            return Err(HandlerError::Empty);
        }
        if s.len() > 39 {
            return Err(HandlerError::TooLong);
        }
        if s == "system" {
            return Err(HandlerError::Reserved);
        }
        for ch in s.chars() {
            if !matches!(ch, 'a'..='z' | '0'..='9' | '-') {
                return Err(HandlerError::InvalidChar(ch));
            }
        }
        if s.starts_with('-') || s.ends_with('-') {
            return Err(HandlerError::HyphenBoundary);
        }
        if s.contains("--") {
            return Err(HandlerError::ConsecutiveHyphens);
        }
        Ok(Self(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Handler {
    type Error = HandlerError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Handler::new(&s)
    }
}

impl From<Handler> for String {
    fn from(h: Handler) -> Self {
        h.0
    }
}

impl std::fmt::Display for Handler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
```

- [ ] **Step 2: Create message types**

`crates/gitim-core/src/types/message.rs`:
```rust
use crate::types::handler::Handler;

/// A parsed message from a .thread file.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub line_number: u64,
    pub point_to: u64,
    pub author: Handler,
    pub timestamp: String,
    pub body: String,
}

/// A line in a .thread file — either a message start or a continuation.
#[derive(Debug, Clone, PartialEq)]
pub enum ThreadLine {
    MessageStart(Message),
    Continuation(String),
}

/// Result of parsing a .thread file.
#[derive(Debug, Clone)]
pub struct ThreadFile {
    pub messages: Vec<Message>,
}
```

- [ ] **Step 3: Create meta types**

`crates/gitim-core/src/types/meta.rs`:
```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UserMeta {
    pub display_name: String,
    pub role: String,
    pub introduction: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelMeta {
    pub display_name: String,
    pub created_by: String,
    pub created_at: String,
    pub introduction: String,
}
```

- [ ] **Step 4: Create config type**

`crates/gitim-core/src/types/config.rs`:
```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    pub version: u32,
    #[serde(default)]
    pub daemon: DaemonConfig,
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
```

- [ ] **Step 5: Create types module**

`crates/gitim-core/src/types.rs`:
```rust
pub mod handler;
pub mod message;
pub mod meta;
pub mod config;

pub use handler::Handler;
pub use message::{Message, ThreadLine, ThreadFile};
pub use meta::{UserMeta, ChannelMeta};
pub use config::Config;
```

- [ ] **Step 6: Verify it compiles**

Run: `cargo check`
Expected: compiles with no errors

- [ ] **Step 7: Commit**

```bash
git add crates/gitim-core/src/
git commit -m "feat(core): define core types — Handler, Message, Meta, Config"
```

---

### Task 0.3: TypeScript CLI Setup

**Files:**
- Create: `cli/package.json`
- Create: `cli/tsconfig.json`
- Create: `cli/src/index.ts`

- [ ] **Step 1: Create package.json**

```json
{
  "name": "gitim-cli",
  "version": "0.1.0",
  "type": "module",
  "bin": {
    "gitim": "./dist/index.js"
  },
  "scripts": {
    "build": "tsc",
    "dev": "tsx src/index.ts"
  },
  "dependencies": {
    "commander": "^13.0.0"
  },
  "devDependencies": {
    "typescript": "^5.7.0",
    "tsx": "^4.19.0",
    "@types/node": "^22.0.0"
  }
}
```

- [ ] **Step 2: Create tsconfig.json**

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "Node16",
    "moduleResolution": "Node16",
    "outDir": "./dist",
    "rootDir": "./src",
    "strict": true,
    "esModuleInterop": true,
    "declaration": true
  },
  "include": ["src"]
}
```

- [ ] **Step 3: Create entry point**

`cli/src/index.ts`:
```typescript
#!/usr/bin/env node
import { Command } from 'commander';

const program = new Command();

program
  .name('gitim')
  .description('GitIM CLI — AI-native IM over Git')
  .version('0.1.0');

program.parse();
```

- [ ] **Step 4: Install and verify**

Run: `cd cli && npm install && npx tsc --noEmit`
Expected: no errors

- [ ] **Step 5: Commit**

```bash
git add cli/
git commit -m "chore: initialize TypeScript CLI project"
```

---

## Chunk 2: Stream A — gitim-core (Parser + Validator)

### Task A.1: Thread File Parser

**Files:**
- Create: `crates/gitim-core/src/parser.rs`
- Create: `crates/gitim-core/tests/parser_test.rs`

- [ ] **Step 1: Write parser tests**

`crates/gitim-core/tests/parser_test.rs`:
```rust
use gitim_core::parser::parse_thread;
use gitim_core::types::Message;

#[test]
fn test_parse_single_message() {
    let input = "[L000001][P000000][@nexus][20250316T120000Z] hello world\n";
    let result = parse_thread(input).unwrap();
    assert_eq!(result.messages.len(), 1);
    let msg = &result.messages[0];
    assert_eq!(msg.line_number, 1);
    assert_eq!(msg.point_to, 0);
    assert_eq!(msg.author.as_str(), "nexus");
    assert_eq!(msg.timestamp, "20250316T120000Z");
    assert_eq!(msg.body, "hello world");
}

#[test]
fn test_parse_message_with_continuation() {
    let input = "[L000001][P000000][@nexus][20250316T120000Z] line one\nline two\nline three\n";
    let result = parse_thread(input).unwrap();
    assert_eq!(result.messages.len(), 1);
    assert_eq!(result.messages[0].body, "line one\nline two\nline three");
}

#[test]
fn test_parse_multiple_messages() {
    let input = "\
[L000001][P000000][@nexus][20250316T120000Z] first
[L000002][P000001][@lewis][20250316T120500Z] reply
";
    let result = parse_thread(input).unwrap();
    assert_eq!(result.messages.len(), 2);
    assert_eq!(result.messages[0].line_number, 1);
    assert_eq!(result.messages[1].line_number, 2);
    assert_eq!(result.messages[1].point_to, 1);
}

#[test]
fn test_parse_mixed_messages_and_continuations() {
    let input = "\
[L000001][P000000][@nexus][20250316T120000Z] multi
continuation line
[L000002][P000001][@lewis][20250316T120500Z] reply
also multi
line three
";
    let result = parse_thread(input).unwrap();
    assert_eq!(result.messages.len(), 2);
    assert_eq!(result.messages[0].body, "multi\ncontinuation line");
    assert_eq!(result.messages[1].body, "reply\nalso multi\nline three");
}

#[test]
fn test_parse_empty_file() {
    let result = parse_thread("").unwrap();
    assert_eq!(result.messages.len(), 0);
}

#[test]
fn test_parse_body_with_brackets() {
    let input = "[L000001][P000000][@nexus][20250316T120000Z] check [this] out\n";
    let result = parse_thread(input).unwrap();
    assert_eq!(result.messages[0].body, "check [this] out");
}

#[test]
fn test_parse_escaped_continuation() {
    // Continuation that starts with [L...] pattern must be escaped with leading space
    let input = "\
[L000001][P000000][@nexus][20250316T120000Z] see example:
 [L000001] this is escaped continuation
";
    let result = parse_thread(input).unwrap();
    assert_eq!(result.messages.len(), 1);
    assert_eq!(result.messages[0].body, "see example:\n[L000001] this is escaped continuation");
}

#[test]
fn test_parse_large_line_numbers() {
    let input = "[L1000000][P0000000][@nexus][20250316T120000Z] big numbers\n";
    let result = parse_thread(input).unwrap();
    assert_eq!(result.messages[0].line_number, 1000000);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p gitim-core --test parser_test`
Expected: FAIL — `parse_thread` not found

- [ ] **Step 3: Implement parser**

`crates/gitim-core/src/parser.rs`:
```rust
use regex::Regex;
use std::sync::LazyLock;
use thiserror::Error;
use crate::types::{Handler, Message, ThreadFile};

#[derive(Error, Debug)]
pub enum ParseError {
    #[error("first non-empty line is not a message start (line {0})")]
    FirstLineNotMessage(usize),
    #[error("invalid handler in message at file line {line}: {source}")]
    InvalidHandler {
        line: usize,
        source: crate::types::handler::HandlerError,
    },
}

static MSG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\[L(\d{6,})\]\[P(\d{6,})\]\[@([a-z0-9-]+)\]\[(\d{8}T\d{6}Z)\] (.+)$").unwrap()
});

pub fn parse_thread(input: &str) -> Result<ThreadFile, ParseError> {
    // Normalize CRLF to LF (spec section 9)
    let input = &input.replace("\r\n", "\n");
    if input.is_empty() {
        return Ok(ThreadFile { messages: vec![] });
    }

    let mut messages: Vec<Message> = Vec::new();
    let mut current_body: Option<String> = None;
    let mut first_content_line = true;

    for (file_line_idx, line) in input.lines().enumerate() {
        if let Some(caps) = MSG_RE.captures(line) {
            // Flush previous message body
            if let (Some(body), Some(msg)) = (current_body.take(), messages.last_mut()) {
                msg.body = body;
            }

            let line_number: u64 = caps[1].parse().unwrap();
            let point_to: u64 = caps[2].parse().unwrap();
            let author = Handler::new(&caps[3]).map_err(|e| ParseError::InvalidHandler {
                line: file_line_idx + 1,
                source: e,
            })?;
            let timestamp = caps[4].to_string();
            let body_first_line = caps[5].to_string();

            messages.push(Message {
                line_number,
                point_to,
                author,
                timestamp,
                body: String::new(),
            });
            current_body = Some(body_first_line);
            first_content_line = false;
        } else {
            // Continuation line
            if first_content_line {
                return Err(ParseError::FirstLineNotMessage(file_line_idx + 1));
            }
            if let Some(ref mut body) = current_body {
                // Strip leading space escape (spec 5.3 rule 5)
                // Strip exactly one leading space if the result matches message prefix (spec 5.3 rule 5)
                let content = if line.starts_with(' ') && MSG_RE.is_match(&line[1..]) {
                    &line[1..]
                } else {
                    line
                };
                body.push('\n');
                body.push_str(content);
            }
        }
    }

    // Flush last message body
    if let (Some(body), Some(msg)) = (current_body, messages.last_mut()) {
        msg.body = body;
    }

    Ok(ThreadFile { messages })
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p gitim-core --test parser_test`
Expected: all 8 tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-core/src/parser.rs crates/gitim-core/tests/
git commit -m "feat(core): implement .thread file parser with continuation support"
```

---

### Task A.2: Handler Validation Tests

**Files:**
- Create: `crates/gitim-core/tests/handler_test.rs`

- [ ] **Step 1: Write handler validation tests**

`crates/gitim-core/tests/handler_test.rs`:
```rust
use gitim_core::types::Handler;

#[test]
fn test_valid_handlers() {
    assert!(Handler::new("nexus").is_ok());
    assert!(Handler::new("lewis").is_ok());
    assert!(Handler::new("cifera-nexus").is_ok());
    assert!(Handler::new("a1").is_ok());
    assert!(Handler::new("x").is_ok());
    assert!(Handler::new("a2b").is_ok());
}

#[test]
fn test_reserved_system() {
    assert!(Handler::new("system").is_err());
}

#[test]
fn test_empty() {
    assert!(Handler::new("").is_err());
}

#[test]
fn test_too_long() {
    let long = "a".repeat(40);
    assert!(Handler::new(&long).is_err());
}

#[test]
fn test_max_length() {
    let max = "a".repeat(39);
    assert!(Handler::new(&max).is_ok());
}

#[test]
fn test_invalid_chars() {
    assert!(Handler::new("NEXUS").is_err());     // uppercase
    assert!(Handler::new("ne xus").is_err());    // space
    assert!(Handler::new("ne_xus").is_err());    // underscore
    assert!(Handler::new("ne.xus").is_err());    // dot
}

#[test]
fn test_hyphen_boundary() {
    assert!(Handler::new("-nexus").is_err());
    assert!(Handler::new("nexus-").is_err());
}

#[test]
fn test_consecutive_hyphens() {
    assert!(Handler::new("ci--fera").is_err());
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p gitim-core --test handler_test`
Expected: all 8 tests PASS (implementation already exists from Task 0.2)

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-core/tests/handler_test.rs
git commit -m "test(core): add handler validation test suite"
```

---

### Task A.3: Meta & Config Validation

**Files:**
- Create: `crates/gitim-core/src/validator.rs`
- Create: `crates/gitim-core/tests/validator_test.rs`

- [ ] **Step 1: Write validator tests**

`crates/gitim-core/tests/validator_test.rs`:
```rust
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p gitim-core --test validator_test`
Expected: FAIL — module not found

- [ ] **Step 3: Implement validator**

Add `serde_yaml` to workspace dependencies in root `Cargo.toml`:
```toml
serde_yaml = "0.9"
```

Add to `crates/gitim-core/Cargo.toml` dependencies:
```toml
serde_yaml.workspace = true
```

`crates/gitim-core/src/validator.rs`:
```rust
use thiserror::Error;
use crate::types::meta::{UserMeta, ChannelMeta};
use crate::types::config::Config;

#[derive(Error, Debug)]
pub enum ValidationError {
    #[error("JSON parse error: {0}")]
    JsonParse(#[from] serde_json::Error),
    #[error("YAML parse error: {0}")]
    YamlParse(#[from] serde_yaml::Error),
    #[error("field '{field}' {reason}")]
    FieldConstraint { field: String, reason: String },
    #[error("invalid channel name: {0}")]
    InvalidChannelName(String),
    #[error("invalid config: {0}")]
    InvalidConfig(String),
}

pub fn validate_user_meta(json: &str) -> Result<UserMeta, ValidationError> {
    let meta: UserMeta = serde_json::from_str(json)?;
    if meta.display_name.is_empty() || meta.display_name.len() > 64 {
        return Err(ValidationError::FieldConstraint {
            field: "display_name".into(),
            reason: "must be 1-64 characters".into(),
        });
    }
    if meta.role.is_empty() || meta.role.len() > 32 {
        return Err(ValidationError::FieldConstraint {
            field: "role".into(),
            reason: "must be 1-32 characters".into(),
        });
    }
    if meta.introduction.is_empty() || meta.introduction.len() > 500 {
        return Err(ValidationError::FieldConstraint {
            field: "introduction".into(),
            reason: "must be 1-500 characters".into(),
        });
    }
    Ok(meta)
}

pub fn validate_channel_meta(json: &str) -> Result<ChannelMeta, ValidationError> {
    let meta: ChannelMeta = serde_json::from_str(json)?;
    if meta.display_name.is_empty() || meta.display_name.len() > 64 {
        return Err(ValidationError::FieldConstraint {
            field: "display_name".into(),
            reason: "must be 1-64 characters".into(),
        });
    }
    if meta.introduction.is_empty() || meta.introduction.len() > 500 {
        return Err(ValidationError::FieldConstraint {
            field: "introduction".into(),
            reason: "must be 1-500 characters".into(),
        });
    }
    // Validate created_by is a valid handler
    use crate::types::Handler;
    Handler::new(&meta.created_by).map_err(|_| ValidationError::FieldConstraint {
        field: "created_by".into(),
        reason: "must be a valid handler".into(),
    })?;
    // Validate created_at format: YYYYMMDDTHHmmssZ
    let ts_re = regex::Regex::new(r"^\d{8}T\d{6}Z$").unwrap();
    if !ts_re.is_match(&meta.created_at) {
        return Err(ValidationError::FieldConstraint {
            field: "created_at".into(),
            reason: "must match YYYYMMDDTHHmmssZ format".into(),
        });
    }
    Ok(meta)
}

pub fn validate_channel_name(name: &str) -> Result<(), ValidationError> {
    if name.is_empty() || name.len() > 32 {
        return Err(ValidationError::InvalidChannelName("must be 1-32 characters".into()));
    }
    if !name.chars().all(|c| matches!(c, 'a'..='z' | '0'..='9' | '-')) {
        return Err(ValidationError::InvalidChannelName("invalid characters".into()));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err(ValidationError::InvalidChannelName("must not start or end with hyphen".into()));
    }
    if name.contains("--") {
        return Err(ValidationError::InvalidChannelName("must not contain consecutive hyphens".into()));
    }
    Ok(())
}

pub fn validate_config(yaml: &str) -> Result<Config, ValidationError> {
    let config: Config = serde_yaml::from_str(yaml)?;
    if config.version != 1 {
        return Err(ValidationError::InvalidConfig(
            format!("unsupported version: {}, expected 1", config.version),
        ));
    }
    Ok(config)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p gitim-core --test validator_test`
Expected: all 10 tests PASS

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/gitim-core/
git commit -m "feat(core): implement meta/config validators with field constraints"
```

---

### Task A.4: Write Validation (Compliance Check)

**Files:**
- Create: `crates/gitim-core/src/validator/compliance.rs`
- Create: `crates/gitim-core/tests/compliance_test.rs`

- [ ] **Step 1: Refactor validator into module**

Convert `crates/gitim-core/src/validator.rs` to `crates/gitim-core/src/validator/mod.rs` and move existing code there. Create `crates/gitim-core/src/validator/compliance.rs`.

- [ ] **Step 2: Write compliance tests**

`crates/gitim-core/tests/compliance_test.rs`:
```rust
use gitim_core::validator::compliance::{validate_append, AppendValidation};

fn make_existing() -> &'static str {
    "[L000001][P000000][@nexus][20250316T120000Z] first message\n\
     [L000002][P000001][@lewis][20250316T120500Z] reply\n"
}

#[test]
fn test_valid_append() {
    let existing = make_existing();
    let new_lines = "[L000003][P000001][@nexus][20250316T121000Z] another reply\n";
    let users = vec!["nexus", "lewis"];
    let result = validate_append(existing, new_lines, &users);
    assert!(result.is_ok());
}

#[test]
fn test_append_wrong_line_number() {
    let existing = make_existing();
    let new_lines = "[L000005][P000001][@nexus][20250316T121000Z] skipped 4\n";
    let users = vec!["nexus"];
    let result = validate_append(existing, new_lines, &users);
    assert!(result.is_err());
}

#[test]
fn test_append_unknown_author() {
    let existing = make_existing();
    let new_lines = "[L000003][P000001][@unknown][20250316T121000Z] who am i\n";
    let users = vec!["nexus", "lewis"];
    let result = validate_append(existing, new_lines, &users);
    assert!(result.is_err());
}

#[test]
fn test_append_invalid_p_reference() {
    let existing = make_existing();
    let new_lines = "[L000003][P000099][@nexus][20250316T121000Z] bad ref\n";
    let users = vec!["nexus"];
    let result = validate_append(existing, new_lines, &users);
    assert!(result.is_err());
}

#[test]
fn test_append_p_references_within_batch() {
    let existing = make_existing();
    let new_lines = "\
[L000003][P000000][@nexus][20250316T121000Z] new topic
[L000004][P000003][@lewis][20250316T121500Z] reply to new topic
";
    let users = vec!["nexus", "lewis"];
    let result = validate_append(existing, new_lines, &users);
    assert!(result.is_ok());
}

#[test]
fn test_append_to_empty_file() {
    let new_lines = "[L000001][P000000][@nexus][20250316T121000Z] first\n";
    let users = vec!["nexus"];
    let result = validate_append("", new_lines, &users);
    assert!(result.is_ok());
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p gitim-core --test compliance_test`
Expected: FAIL — module not found

- [ ] **Step 4: Implement compliance validator**

`crates/gitim-core/src/validator/compliance.rs`:
```rust
use crate::parser::parse_thread;
use thiserror::Error;
use std::collections::HashSet;

#[derive(Error, Debug)]
pub enum ComplianceError {
    #[error("parse error: {0}")]
    Parse(#[from] crate::parser::ParseError),
    #[error("line number not continuous: expected L{expected:06}, got L{got:06}")]
    LineNumberGap { expected: u64, got: u64 },
    #[error("unknown author '@{0}' not in users/")]
    UnknownAuthor(String),
    #[error("invalid P reference: P{0:06} does not exist")]
    InvalidPointTo(u64),
    #[error("message L{0:06} has empty body")]
    EmptyBody(u64),
}

pub struct AppendValidation;

pub fn validate_append(
    existing: &str,
    new_lines: &str,
    registered_users: &[&str],
) -> Result<AppendValidation, ComplianceError> {
    let existing_file = parse_thread(existing)?;
    let new_file = parse_thread(new_lines)?;

    let max_existing = existing_file
        .messages
        .last()
        .map(|m| m.line_number)
        .unwrap_or(0);

    // Collect all known line numbers (existing + new as we validate)
    let mut known_lines: HashSet<u64> = existing_file
        .messages
        .iter()
        .map(|m| m.line_number)
        .collect();

    let user_set: HashSet<&str> = registered_users.iter().copied().collect();

    let mut expected_next = max_existing + 1;

    for msg in &new_file.messages {
        // Check line number continuity
        if msg.line_number != expected_next {
            return Err(ComplianceError::LineNumberGap {
                expected: expected_next,
                got: msg.line_number,
            });
        }

        // Check author
        if !user_set.contains(msg.author.as_str()) {
            return Err(ComplianceError::UnknownAuthor(msg.author.to_string()));
        }

        // Check P reference (P000000 is always valid)
        if msg.point_to != 0 && !known_lines.contains(&msg.point_to) {
            return Err(ComplianceError::InvalidPointTo(msg.point_to));
        }

        // Check non-empty body (spec section 9)
        if msg.body.trim().is_empty() {
            return Err(ComplianceError::EmptyBody(msg.line_number));
        }

        known_lines.insert(msg.line_number);
        expected_next = msg.line_number + 1;
    }

    Ok(AppendValidation)
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p gitim-core --test compliance_test`
Expected: all 6 tests PASS

- [ ] **Step 6: Commit**

```bash
git add crates/gitim-core/src/validator/ crates/gitim-core/tests/compliance_test.rs
git commit -m "feat(core): implement write-path compliance validation"
```

---

### Task A.5: Message Formatter (Write Path)

**Files:**
- Create: `crates/gitim-core/src/formatter.rs`
- Create: `crates/gitim-core/tests/formatter_test.rs`

- [ ] **Step 1: Write formatter tests**

`crates/gitim-core/tests/formatter_test.rs`:
```rust
use gitim_core::formatter::format_message;
use gitim_core::types::Handler;

#[test]
fn test_format_simple_message() {
    let result = format_message(1, 0, &Handler::new("nexus").unwrap(), "20250316T120000Z", "hello");
    assert_eq!(result, "[L000001][P000000][@nexus][20250316T120000Z] hello\n");
}

#[test]
fn test_format_reply() {
    let result = format_message(5, 3, &Handler::new("lewis").unwrap(), "20250316T120000Z", "reply");
    assert_eq!(result, "[L000005][P000003][@lewis][20250316T120000Z] reply\n");
}

#[test]
fn test_format_multiline_body() {
    let result = format_message(1, 0, &Handler::new("nexus").unwrap(), "20250316T120000Z", "line one\nline two\nline three");
    assert_eq!(result, "[L000001][P000000][@nexus][20250316T120000Z] line one\nline two\nline three\n");
}

#[test]
fn test_format_body_needing_escape() {
    let body = "[L000001] looks like a message prefix";
    let result = format_message(2, 0, &Handler::new("nexus").unwrap(), "20250316T120000Z", &format!("see:\n{}", body));
    // The continuation line starting with [L000001] must get a leading space
    assert!(result.contains("\n [L000001]"));
}

#[test]
fn test_format_large_line_number() {
    let result = format_message(1000000, 0, &Handler::new("nexus").unwrap(), "20250316T120000Z", "big");
    assert_eq!(result, "[L1000000][P0000000][@nexus][20250316T120000Z] big\n");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p gitim-core --test formatter_test`
Expected: FAIL — module not found

- [ ] **Step 3: Implement formatter**

`crates/gitim-core/src/formatter.rs`:
```rust
use regex::Regex;
use std::sync::LazyLock;
use crate::types::Handler;

static MSG_PREFIX_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\[L\d{6,}\]\[P\d{6,}\]\[@[a-z0-9-]+\]\[\d{8}T\d{6}Z\] ").unwrap()
});

pub fn format_message(
    line_number: u64,
    point_to: u64,
    author: &Handler,
    timestamp: &str,
    body: &str,
) -> String {
    let width = format!("{}", line_number).len().max(6);
    let mut output = format!(
        "[L{:0>width$}][P{:0>width$}][@{}][{}] ",
        line_number,
        point_to,
        author.as_str(),
        timestamp,
        width = width,
    );

    let mut lines = body.lines().peekable();
    if let Some(first) = lines.next() {
        output.push_str(first);
        output.push('\n');

        for line in lines {
            // Escape continuation lines that look like message prefixes
            if MSG_PREFIX_RE.is_match(line) {
                output.push(' ');
            }
            output.push_str(line);
            output.push('\n');
        }
    } else {
        output.push('\n');
    }

    output
}
```

Update `crates/gitim-core/src/lib.rs`:
```rust
pub mod types;
pub mod parser;
pub mod validator;
pub mod formatter;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p gitim-core --test formatter_test`
Expected: all 5 tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-core/src/formatter.rs crates/gitim-core/src/lib.rs crates/gitim-core/tests/formatter_test.rs
git commit -m "feat(core): implement message formatter with continuation escaping"
```

---

## Chunk 3: Stream B — gitim-daemon (Server)

### Task B.1: Daemon Lifecycle (PID/Lock/Socket)

**Files:**
- Create: `crates/gitim-daemon/src/lifecycle.rs`
- Create: `crates/gitim-daemon/src/error.rs`

- [ ] **Step 1: Implement error types**

`crates/gitim-daemon/src/error.rs`:
```rust
use thiserror::Error;

#[derive(Error, Debug)]
pub enum DaemonError {
    #[error("daemon already running (pid: {0})")]
    AlreadyRunning(u32),
    #[error("failed to acquire lock: {0}")]
    LockFailed(#[from] std::io::Error),
    #[error("gitim repo not found at {0}")]
    RepoNotFound(String),
    #[error("invalid config: {0}")]
    ConfigError(String),
}
```

- [ ] **Step 2: Implement lifecycle manager**

`crates/gitim-daemon/src/lifecycle.rs`:
```rust
use std::fs;
use std::path::{Path, PathBuf};
use crate::error::DaemonError;

pub struct DaemonLifecycle {
    run_dir: PathBuf,
}

impl DaemonLifecycle {
    pub fn new(repo_root: &Path) -> Self {
        Self {
            run_dir: repo_root.join(".gitim").join("run"),
        }
    }

    pub fn ensure_run_dir(&self) -> Result<(), DaemonError> {
        fs::create_dir_all(&self.run_dir)?;
        Ok(())
    }

    pub fn is_running(&self) -> Option<u32> {
        let pid_file = self.run_dir.join("gitim.pid");
        let pid_str = fs::read_to_string(&pid_file).ok()?;
        let pid: u32 = pid_str.trim().parse().ok()?;
        // Check if process exists
        if process_exists(pid) {
            Some(pid)
        } else {
            // Stale pid file, clean up
            let _ = fs::remove_file(&pid_file);
            None
        }
    }

    pub fn write_pid(&self) -> Result<(), DaemonError> {
        let pid = std::process::id();
        fs::write(self.run_dir.join("gitim.pid"), pid.to_string())?;
        Ok(())
    }

    pub fn socket_path(&self) -> PathBuf {
        self.run_dir.join("gitim.sock")
    }

    pub fn write_port(&self, port: u16) -> Result<(), DaemonError> {
        fs::write(self.run_dir.join("gitim.port"), port.to_string())?;
        Ok(())
    }

    pub fn cleanup(&self) {
        let _ = fs::remove_file(self.run_dir.join("gitim.pid"));
        let _ = fs::remove_file(self.run_dir.join("gitim.sock"));
        let _ = fs::remove_file(self.run_dir.join("gitim.port"));
        let _ = fs::remove_file(self.run_dir.join("gitim.lock"));
    }
}

#[cfg(unix)]
fn process_exists(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(not(unix))]
fn process_exists(_pid: u32) -> bool {
    false
}
```

Add `libc = "0.2"` to `crates/gitim-daemon/Cargo.toml`.

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p gitim-daemon`
Expected: compiles

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-daemon/
git commit -m "feat(daemon): implement lifecycle manager — PID, lock, socket, cleanup"
```

---

### Task B.2: Unix Socket Server + JSON API

**Files:**
- Create: `crates/gitim-daemon/src/api.rs`
- Create: `crates/gitim-daemon/src/server.rs`

- [ ] **Step 1: Define API request/response types**

`crates/gitim-daemon/src/api.rs`:
```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(tag = "method")]
pub enum Request {
    #[serde(rename = "send")]
    Send {
        channel: String,
        body: String,
        reply_to: Option<u64>,
        author: String,
    },
    #[serde(rename = "read")]
    Read {
        channel: String,
        limit: Option<usize>,
        since: Option<u64>,
    },
    #[serde(rename = "channels")]
    ListChannels,
    #[serde(rename = "users")]
    ListUsers,
    #[serde(rename = "thread")]
    GetThread {
        channel: String,
        line_number: u64,
    },
    #[serde(rename = "status")]
    Status,
}

#[derive(Debug, Serialize)]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn success(data: serde_json::Value) -> Self {
        Self { ok: true, data: Some(data), error: None }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self { ok: false, data: None, error: Some(msg.into()) }
    }
}
```

- [ ] **Step 2: Implement socket server**

`crates/gitim-daemon/src/server.rs`:
```rust
use std::path::Path;
use tokio::net::UnixListener;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{info, error};
use crate::api::{Request, Response};

pub async fn start_unix_socket(socket_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    // Remove stale socket
    let _ = std::fs::remove_file(socket_path);

    let listener = UnixListener::bind(socket_path)?;
    info!("listening on {:?}", socket_path);

    loop {
        let (stream, _) = listener.accept().await?;
        tokio::spawn(async move {
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            let mut line = String::new();

            while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
                let response = match serde_json::from_str::<Request>(&line) {
                    Ok(req) => handle_request(req).await,
                    Err(e) => Response::error(format!("invalid request: {}", e)),
                };

                let mut resp_json = serde_json::to_string(&response).unwrap();
                resp_json.push('\n');
                if let Err(e) = writer.write_all(resp_json.as_bytes()).await {
                    error!("write error: {}", e);
                    break;
                }
                line.clear();
            }
        });
    }
}

async fn handle_request(req: Request) -> Response {
    match req {
        Request::Status => Response::success(serde_json::json!({
            "version": "0.1.0",
            "status": "running",
        })),
        // Other handlers will be implemented as Stream A/C provide the logic
        _ => Response::error("not implemented yet"),
    }
}
```

- [ ] **Step 3: Wire up main.rs**

`crates/gitim-daemon/src/main.rs`:
```rust
mod api;
mod error;
mod lifecycle;
mod server;

use std::path::PathBuf;
use tracing_subscriber;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::init();

    // For now, use current directory as repo root
    let repo_root = PathBuf::from(".");
    let lifecycle = lifecycle::DaemonLifecycle::new(&repo_root);

    if let Some(pid) = lifecycle.is_running() {
        eprintln!("daemon already running (pid: {})", pid);
        std::process::exit(1);
    }

    lifecycle.ensure_run_dir()?;
    lifecycle.write_pid()?;

    // Cleanup on shutdown
    let lc = lifecycle::DaemonLifecycle::new(&repo_root);
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        lc.cleanup();
        std::process::exit(0);
    });

    let socket_path = lifecycle.socket_path();
    server::start_unix_socket(&socket_path).await?;

    Ok(())
}
```

Uses `tokio::signal::ctrl_c()` for async-compatible shutdown handling (already in tokio's `full` feature set).

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p gitim-daemon`
Expected: compiles

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-daemon/
git commit -m "feat(daemon): implement Unix socket server with JSON API framework"
```

---

### Task B.3: HTTP Debug Server

**Files:**
- Create: `crates/gitim-daemon/src/http.rs`

- [ ] **Step 1: Implement HTTP debug server using axum**

`crates/gitim-daemon/src/http.rs`:
```rust
use axum::{Router, Json, routing::post};
use crate::api::{Request, Response};

pub fn create_router() -> Router {
    Router::new()
        .route("/api", post(handle_api))
}

async fn handle_api(Json(req): Json<Request>) -> Json<Response> {
    // Reuse same handler as Unix socket
    let response = match req {
        Request::Status => Response::success(serde_json::json!({
            "version": "0.1.0",
            "status": "running",
        })),
        _ => Response::error("not implemented yet"),
    };
    Json(response)
}
```

- [ ] **Step 2: Wire HTTP server into main.rs (conditional on config)**

Update `main.rs` to optionally start HTTP server alongside the Unix socket server based on `config.yaml` `daemon.debug_http` setting.

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p gitim-daemon`
Expected: compiles

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-daemon/
git commit -m "feat(daemon): add optional HTTP debug server via axum"
```

---

## Chunk 4: Stream C — gitim-sync (Git Engine)

### Task C.1: Git Operations

**Files:**
- Create: `crates/gitim-sync/src/git.rs`
- Create: `crates/gitim-sync/tests/git_test.rs`

- [ ] **Step 1: Implement git operations wrapper**

`crates/gitim-sync/src/git.rs`:
```rust
use std::path::Path;
use std::process::Command;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum GitError {
    #[error("git command failed: {0}")]
    CommandFailed(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("push failed after {0} retries")]
    PushRetriesExhausted(u32),
}

pub struct GitRepo {
    root: std::path::PathBuf,
}

impl GitRepo {
    pub fn new(root: &Path) -> Self {
        Self { root: root.to_path_buf() }
    }

    pub fn pull_rebase(&self) -> Result<(), GitError> {
        let output = Command::new("git")
            .args(["pull", "--rebase"])
            .current_dir(&self.root)
            .output()?;
        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        Ok(())
    }

    pub fn add_and_commit(&self, paths: &[&str], message: &str) -> Result<(), GitError> {
        let mut args = vec!["add"];
        args.extend(paths);
        let output = Command::new("git")
            .args(&args)
            .current_dir(&self.root)
            .output()?;
        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let output = Command::new("git")
            .args(["commit", "-m", message])
            .current_dir(&self.root)
            .output()?;
        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        Ok(())
    }

    pub fn push(&self) -> Result<(), GitError> {
        let output = Command::new("git")
            .args(["push"])
            .current_dir(&self.root)
            .output()?;
        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        Ok(())
    }

    pub fn push_with_retry(&self, max_retries: u32) -> Result<(), GitError> {
        for attempt in 0..=max_retries {
            match self.push() {
                Ok(()) => return Ok(()),
                Err(_) if attempt < max_retries => {
                    self.pull_rebase()?;
                    // Caller is responsible for re-numbering and re-committing
                    // before calling push_with_retry again
                }
                Err(_) => return Err(GitError::PushRetriesExhausted(max_retries)),
            }
        }
        Err(GitError::PushRetriesExhausted(max_retries))
    }

    pub fn has_remote(&self) -> bool {
        Command::new("git")
            .args(["remote", "get-url", "origin"])
            .current_dir(&self.root)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p gitim-sync`
Expected: compiles

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-sync/
git commit -m "feat(sync): implement git operations wrapper — pull, push, commit, retry"
```

---

### Task C.2: File Watcher

**Files:**
- Create: `crates/gitim-sync/src/watcher.rs`

- [ ] **Step 1: Implement file watcher**

Add `notify = "7"` to `crates/gitim-sync/Cargo.toml`.

`crates/gitim-sync/src/watcher.rs`:
```rust
use notify::{Watcher, RecursiveMode, Event, EventKind};
use std::path::Path;
use tokio::sync::mpsc;
use tracing::info;

pub enum FileEvent {
    ThreadModified(String),  // channel name or dm name
    MetaModified(String),
}

pub async fn watch_repo(
    repo_root: &Path,
    tx: mpsc::Sender<FileEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (notify_tx, mut notify_rx) = tokio::sync::mpsc::channel(100);

    let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
        if let Ok(event) = res {
            if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                let _ = notify_tx.blocking_send(event);
            }
        }
    })?;

    let channels_dir = repo_root.join("channels");
    let dm_dir = repo_root.join("dm");

    if channels_dir.exists() {
        watcher.watch(&channels_dir, RecursiveMode::NonRecursive)?;
    }
    if dm_dir.exists() {
        watcher.watch(&dm_dir, RecursiveMode::NonRecursive)?;
    }

    info!("file watcher started");

    // Keep watcher alive and forward events
    tokio::spawn(async move {
        let _watcher = watcher; // prevent drop
        while let Some(event) = notify_rx.recv().await {
            for path in event.paths {
                let filename = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
                if filename.ends_with(".thread") {
                    let name = filename.trim_end_matches(".thread").to_string();
                    let _ = tx.send(FileEvent::ThreadModified(name)).await;
                } else if filename.ends_with(".meta.json") {
                    let name = filename.trim_end_matches(".meta.json").to_string();
                    let _ = tx.send(FileEvent::MetaModified(name)).await;
                }
            }
        }
    });

    Ok(())
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p gitim-sync`
Expected: compiles

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-sync/
git commit -m "feat(sync): implement file watcher for .thread and .meta.json changes"
```

---

### Task C.3: Sync Loop

**Files:**
- Create: `crates/gitim-sync/src/sync_loop.rs`

- [ ] **Step 1: Implement periodic sync**

`crates/gitim-sync/src/sync_loop.rs`:
```rust
use std::path::Path;
use std::time::Duration;
use tokio::time;
use tracing::{info, warn};
use crate::git::GitRepo;

pub async fn start_sync_loop(repo_root: &Path, interval_secs: u32) {
    if interval_secs == 0 {
        info!("sync_interval=0, auto-sync disabled");
        return;
    }

    let repo = GitRepo::new(repo_root);

    if !repo.has_remote() {
        info!("no remote configured, sync loop disabled");
        return;
    }

    let interval = Duration::from_secs(interval_secs as u64);
    info!("sync loop started, interval={}s", interval_secs);

    let mut ticker = time::interval(interval);
    ticker.tick().await; // skip first immediate tick

    loop {
        ticker.tick().await;
        match repo.pull_rebase() {
            Ok(()) => info!("sync: pull complete"),
            Err(e) => warn!("sync: pull failed: {}", e),
        }
    }
}
```

Update `crates/gitim-sync/src/lib.rs`:
```rust
pub mod git;
pub mod watcher;
pub mod sync_loop;
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p gitim-sync`
Expected: compiles

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-sync/
git commit -m "feat(sync): implement periodic git pull sync loop"
```

---

## Chunk 5: Stream D — gitim-cli (TypeScript CLI)

### Task D.1: Socket Client Library

**Files:**
- Create: `cli/src/client.ts`

- [ ] **Step 1: Implement socket client**

`cli/src/client.ts`:
```typescript
import net from 'node:net';
import fs from 'node:fs';
import path from 'node:path';
import readline from 'node:readline';

export interface ApiResponse {
  ok: boolean;
  data?: any;
  error?: string;
}

export class GitimClient {
  private socketPath: string;

  constructor(repoRoot: string) {
    this.socketPath = path.join(repoRoot, '.gitim', 'run', 'gitim.sock');
  }

  async request(method: string, params: Record<string, any> = {}): Promise<ApiResponse> {
    return new Promise((resolve, reject) => {
      const socket = net.createConnection(this.socketPath);
      const payload = JSON.stringify({ method, ...params }) + '\n';

      socket.on('connect', () => {
        socket.write(payload);
      });

      const rl = readline.createInterface({ input: socket });
      rl.on('line', (line: string) => {
        try {
          resolve(JSON.parse(line));
        } catch {
          reject(new Error(`Invalid response: ${line}`));
        }
        socket.end();
      });

      socket.on('error', (err: Error) => {
        reject(new Error(`Cannot connect to daemon: ${err.message}`));
      });
    });
  }

  async status(): Promise<ApiResponse> {
    return this.request('status');
  }

  async send(channel: string, body: string, author: string, replyTo?: number): Promise<ApiResponse> {
    return this.request('send', { channel, body, author, reply_to: replyTo ?? null });
  }

  async read(channel: string, limit?: number, since?: number): Promise<ApiResponse> {
    return this.request('read', { channel, limit: limit ?? null, since: since ?? null });
  }

  async listChannels(): Promise<ApiResponse> {
    return this.request('channels');
  }

  async listUsers(): Promise<ApiResponse> {
    return this.request('users');
  }

  async getThread(channel: string, lineNumber: number): Promise<ApiResponse> {
    return this.request('thread', { channel, line_number: lineNumber });
  }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cd cli && npx tsc --noEmit`
Expected: no errors

- [ ] **Step 3: Commit**

```bash
git add cli/src/client.ts
git commit -m "feat(cli): implement Unix socket client library"
```

---

### Task D.2: Daemon Auto-Launch

**Files:**
- Create: `cli/src/daemon.ts`

- [ ] **Step 1: Implement daemon launcher**

`cli/src/daemon.ts`:
```typescript
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

const DAEMON_STARTUP_TIMEOUT_MS = 5000;
const POLL_INTERVAL_MS = 100;

export function findRepoRoot(from: string = process.cwd()): string | null {
  let dir = from;
  while (true) {
    if (fs.existsSync(path.join(dir, '.gitim', 'config.yaml'))) {
      return dir;
    }
    const parent = path.dirname(dir);
    if (parent === dir) return null;
    dir = parent;
  }
}

export function isDaemonRunning(repoRoot: string): boolean {
  const pidFile = path.join(repoRoot, '.gitim', 'run', 'gitim.pid');
  if (!fs.existsSync(pidFile)) return false;
  const pid = parseInt(fs.readFileSync(pidFile, 'utf-8').trim(), 10);
  if (isNaN(pid)) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

export async function ensureDaemon(repoRoot: string): Promise<void> {
  if (isDaemonRunning(repoRoot)) return;

  const child = spawn('gitim-daemon', [], {
    cwd: repoRoot,
    detached: true,
    stdio: 'ignore',
  });
  child.unref();

  // Wait for socket to appear
  const sockPath = path.join(repoRoot, '.gitim', 'run', 'gitim.sock');
  const deadline = Date.now() + DAEMON_STARTUP_TIMEOUT_MS;

  while (Date.now() < deadline) {
    if (fs.existsSync(sockPath)) return;
    await new Promise((r) => setTimeout(r, POLL_INTERVAL_MS));
  }

  throw new Error('daemon failed to start within timeout');
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cd cli && npx tsc --noEmit`
Expected: no errors

- [ ] **Step 3: Commit**

```bash
git add cli/src/daemon.ts
git commit -m "feat(cli): implement daemon auto-launch with repo root discovery"
```

---

### Task D.3: CLI Commands

**Files:**
- Modify: `cli/src/index.ts`
- Create: `cli/src/commands/send.ts`
- Create: `cli/src/commands/read.ts`
- Create: `cli/src/commands/channels.ts`
- Create: `cli/src/commands/users.ts`
- Create: `cli/src/commands/init.ts`
- Create: `cli/src/commands/status.ts`

- [ ] **Step 1: Implement init command**

`cli/src/commands/init.ts`:
```typescript
import fs from 'node:fs';
import path from 'node:path';

export function initRepo(dir: string = process.cwd()): void {
  const dirs = [
    path.join(dir, '.gitim'),
    path.join(dir, 'users'),
    path.join(dir, 'channels'),
  ];

  for (const d of dirs) {
    fs.mkdirSync(d, { recursive: true });
  }

  const configPath = path.join(dir, '.gitim', 'config.yaml');
  if (!fs.existsSync(configPath)) {
    fs.writeFileSync(configPath, 'version: 1\n');
  }

  const gitignorePath = path.join(dir, '.gitignore');
  const gitignoreContent = fs.existsSync(gitignorePath)
    ? fs.readFileSync(gitignorePath, 'utf-8')
    : '';
  if (!gitignoreContent.includes('.gitim/run/')) {
    fs.appendFileSync(gitignorePath, '\n.gitim/run/\n');
  }

  console.log('GitIM repository initialized.');
}
```

- [ ] **Step 2: Implement remaining commands (send, read, channels, users, status)**

Each command follows the same pattern:
1. Find repo root via `findRepoRoot()`
2. Ensure daemon is running via `ensureDaemon()`
3. Create client via `new GitimClient(repoRoot)`
4. Call appropriate client method
5. Print result

- [ ] **Step 3: Wire commands into index.ts**

Update `cli/src/index.ts` to register all subcommands with commander.

- [ ] **Step 4: Verify it compiles**

Run: `cd cli && npx tsc --noEmit`
Expected: no errors

- [ ] **Step 5: Commit**

```bash
git add cli/src/
git commit -m "feat(cli): implement all v1 commands — init, send, read, channels, users, status"
```

---

## Chunk 6: Stream A Supplement — DM & Read-Path Validation

### Task A.6: DM Filename Utilities

**Files:**
- Create: `crates/gitim-core/src/dm.rs`
- Create: `crates/gitim-core/tests/dm_test.rs`

- [ ] **Step 1: Write DM filename tests**

`crates/gitim-core/tests/dm_test.rs`:
```rust
use gitim_core::dm::dm_filename;
use gitim_core::types::Handler;

#[test]
fn test_dm_filename_ordering() {
    let a = Handler::new("lewis").unwrap();
    let b = Handler::new("nexus").unwrap();
    assert_eq!(dm_filename(&a, &b), "lewis--nexus");
    assert_eq!(dm_filename(&b, &a), "lewis--nexus"); // order doesn't matter
}

#[test]
fn test_dm_filename_with_hyphens() {
    let a = Handler::new("cifera-nexus").unwrap();
    let b = Handler::new("lewis").unwrap();
    assert_eq!(dm_filename(&a, &b), "cifera-nexus--lewis");
}

#[test]
fn test_dm_filename_prefix_match() {
    let a = Handler::new("alice").unwrap();
    let b = Handler::new("alice2").unwrap();
    assert_eq!(dm_filename(&a, &b), "alice--alice2");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p gitim-core --test dm_test`
Expected: FAIL

- [ ] **Step 3: Implement DM utilities**

`crates/gitim-core/src/dm.rs`:
```rust
use crate::types::Handler;

/// Generate DM filename stem (without extension) from two handlers.
/// Handlers are sorted lexicographically and joined with `--`.
pub fn dm_filename(a: &Handler, b: &Handler) -> String {
    let (first, second) = if a.as_str() <= b.as_str() {
        (a.as_str(), b.as_str())
    } else {
        (b.as_str(), a.as_str())
    };
    format!("{}--{}", first, second)
}

/// Parse a DM filename stem back into two handler strings.
/// Returns None if the filename does not contain `--`.
pub fn parse_dm_filename(stem: &str) -> Option<(&str, &str)> {
    let idx = stem.find("--")?;
    let first = &stem[..idx];
    let second = &stem[idx + 2..];
    if first.is_empty() || second.is_empty() {
        return None;
    }
    Some((first, second))
}
```

Update `crates/gitim-core/src/lib.rs` to add `pub mod dm;`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p gitim-core --test dm_test`
Expected: all 3 tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-core/src/dm.rs crates/gitim-core/src/lib.rs crates/gitim-core/tests/dm_test.rs
git commit -m "feat(core): implement DM filename generation and parsing"
```

---

### Task A.7: Read-Path Compliance Detection

**Files:**
- Create: `crates/gitim-core/src/validator/read_check.rs`
- Create: `crates/gitim-core/tests/read_check_test.rs`

- [ ] **Step 1: Write read-path detection tests**

`crates/gitim-core/tests/read_check_test.rs`:
```rust
use gitim_core::validator::read_check::{check_thread_integrity, IntegrityIssue};

#[test]
fn test_clean_thread() {
    let input = "\
[L000001][P000000][@nexus][20250316T120000Z] hello
[L000002][P000001][@lewis][20250316T120500Z] reply
";
    let users = vec!["nexus", "lewis"];
    let issues = check_thread_integrity(input, &users);
    assert!(issues.is_empty());
}

#[test]
fn test_detect_gap() {
    let input = "\
[L000001][P000000][@nexus][20250316T120000Z] hello
[L000003][P000001][@lewis][20250316T120500Z] skipped 2
";
    let users = vec!["nexus", "lewis"];
    let issues = check_thread_integrity(input, &users);
    assert!(issues.iter().any(|i| matches!(i, IntegrityIssue::LineNumberGap { .. })));
}

#[test]
fn test_detect_unknown_author() {
    let input = "[L000001][P000000][@unknown][20250316T120000Z] who\n";
    let users = vec!["nexus"];
    let issues = check_thread_integrity(input, &users);
    assert!(issues.iter().any(|i| matches!(i, IntegrityIssue::UnknownAuthor(_))));
}

#[test]
fn test_detect_invalid_p_ref() {
    let input = "[L000001][P000099][@nexus][20250316T120000Z] bad ref\n";
    let users = vec!["nexus"];
    let issues = check_thread_integrity(input, &users);
    assert!(issues.iter().any(|i| matches!(i, IntegrityIssue::InvalidPointTo(_))));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p gitim-core --test read_check_test`
Expected: FAIL

- [ ] **Step 3: Implement read-path checker**

`crates/gitim-core/src/validator/read_check.rs`:
```rust
use crate::parser::parse_thread;
use std::collections::HashSet;

/// Issues found during read-path integrity checking.
/// These are warnings, not hard errors — the data is preserved.
#[derive(Debug)]
pub enum IntegrityIssue {
    LineNumberGap { expected: u64, got: u64 },
    UnknownAuthor(String),
    InvalidPointTo(u64),
    EmptyBody(u64),
    ParseError(String),
}

/// Check a thread file's integrity. Returns a list of issues found.
/// An empty list means the file is fully compliant.
pub fn check_thread_integrity(input: &str, registered_users: &[&str]) -> Vec<IntegrityIssue> {
    let mut issues = Vec::new();

    let file = match parse_thread(input) {
        Ok(f) => f,
        Err(e) => {
            issues.push(IntegrityIssue::ParseError(e.to_string()));
            return issues;
        }
    };

    let user_set: HashSet<&str> = registered_users.iter().copied().collect();
    let mut known_lines: HashSet<u64> = HashSet::new();
    let mut expected_next: u64 = 1;

    for msg in &file.messages {
        if msg.line_number != expected_next {
            issues.push(IntegrityIssue::LineNumberGap {
                expected: expected_next,
                got: msg.line_number,
            });
        }

        if !user_set.contains(msg.author.as_str()) {
            issues.push(IntegrityIssue::UnknownAuthor(msg.author.to_string()));
        }

        if msg.point_to != 0 && !known_lines.contains(&msg.point_to) {
            issues.push(IntegrityIssue::InvalidPointTo(msg.point_to));
        }

        if msg.body.trim().is_empty() {
            issues.push(IntegrityIssue::EmptyBody(msg.line_number));
        }

        known_lines.insert(msg.line_number);
        expected_next = msg.line_number + 1;
    }

    issues
}
```

Update `crates/gitim-core/src/validator/mod.rs` to add `pub mod read_check;`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p gitim-core --test read_check_test`
Expected: all 4 tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-core/src/validator/ crates/gitim-core/tests/read_check_test.rs
git commit -m "feat(core): implement read-path integrity checking for pulled content"
```

---

## Chunk 7: Stream C Supplement — Conflict Re-Numbering

### Task C.4: Conflict Resolution with Line Re-Numbering

**Files:**
- Create: `crates/gitim-sync/src/renumber.rs`
- Create: `crates/gitim-sync/tests/renumber_test.rs`

- [ ] **Step 1: Write re-numbering tests**

`crates/gitim-sync/tests/renumber_test.rs`:
```rust
use gitim_sync::renumber::renumber_batch;

#[test]
fn test_renumber_simple() {
    let batch = "\
[L000003][P000000][@nexus][20250316T120000Z] new topic
[L000004][P000003][@lewis][20250316T120500Z] reply
";
    // After rebase, max existing is now 5 (someone else pushed 3,4,5)
    let result = renumber_batch(batch, 5).unwrap();
    assert!(result.contains("[L000006]"));
    assert!(result.contains("[L000007]"));
    // P000000 stays P000000 (top-level)
    assert!(result.contains("[P000000]"));
    // P000003 was intra-batch ref to old L000003, should become P000006
    assert!(result.contains("[P000006]"));
}

#[test]
fn test_renumber_preserves_external_refs() {
    let batch = "[L000003][P000002][@nexus][20250316T120000Z] reply to existing\n";
    // P000002 references a pre-existing line, should NOT change
    let result = renumber_batch(batch, 5).unwrap();
    assert!(result.contains("[L000006]"));
    assert!(result.contains("[P000002]")); // preserved
}

#[test]
fn test_renumber_with_continuations() {
    let batch = "\
[L000003][P000000][@nexus][20250316T120000Z] multi
continuation line
[L000004][P000003][@lewis][20250316T120500Z] reply
";
    let result = renumber_batch(batch, 10).unwrap();
    assert!(result.contains("[L000011]"));
    assert!(result.contains("continuation line"));
    assert!(result.contains("[L000012]"));
    assert!(result.contains("[P000011]")); // intra-batch ref updated
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p gitim-sync --test renumber_test`
Expected: FAIL

- [ ] **Step 3: Implement re-numbering**

`crates/gitim-sync/src/renumber.rs`:
```rust
use gitim_core::parser::parse_thread;
use gitim_core::formatter::format_message;
use std::collections::HashMap;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RenumberError {
    #[error("parse error: {0}")]
    Parse(#[from] gitim_core::parser::ParseError),
}

/// Re-number a batch of messages starting from `new_start` (= max_existing + 1).
/// Updates intra-batch P references. External P references are preserved.
pub fn renumber_batch(batch: &str, max_existing: u64) -> Result<String, RenumberError> {
    let file = parse_thread(batch)?;

    // Build mapping: old line number -> new line number
    let mut line_map: HashMap<u64, u64> = HashMap::new();
    let batch_line_numbers: std::collections::HashSet<u64> =
        file.messages.iter().map(|m| m.line_number).collect();

    for (i, msg) in file.messages.iter().enumerate() {
        line_map.insert(msg.line_number, max_existing + 1 + i as u64);
    }

    // Rebuild batch with new line numbers
    let mut output = String::new();
    for msg in &file.messages {
        let new_ln = line_map[&msg.line_number];
        let new_pt = if msg.point_to == 0 {
            0 // top-level stays top-level
        } else if batch_line_numbers.contains(&msg.point_to) {
            // Intra-batch ref: remap
            line_map[&msg.point_to]
        } else {
            // External ref: preserve
            msg.point_to
        };

        output.push_str(&format_message(
            new_ln,
            new_pt,
            &msg.author,
            &msg.timestamp,
            &msg.body,
        ));
    }

    Ok(output)
}
```

Add `gitim-core = { path = "../gitim-core" }` to `crates/gitim-sync/Cargo.toml` if not already present.

Update `crates/gitim-sync/src/lib.rs`:
```rust
pub mod git;
pub mod watcher;
pub mod sync_loop;
pub mod renumber;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p gitim-sync --test renumber_test`
Expected: all 3 tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-sync/
git commit -m "feat(sync): implement conflict resolution line re-numbering"
```

---

## Chunk 8: Integration & Wiring

### Task I.1: Daemon Shared State & Config Loading

**Files:**
- Create: `crates/gitim-daemon/src/state.rs`
- Modify: `crates/gitim-daemon/src/main.rs`

- [ ] **Step 1: Create shared state struct**

`crates/gitim-daemon/src/state.rs`:
```rust
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use gitim_core::types::{Config, ThreadFile};

pub type SharedState = Arc<AppState>;

pub struct AppState {
    pub repo_root: PathBuf,
    pub config: Config,
    pub thread_cache: RwLock<HashMap<String, ThreadFile>>,
    pub users: RwLock<Vec<String>>,
}

impl AppState {
    pub fn new(repo_root: PathBuf, config: Config) -> Self {
        Self {
            repo_root,
            config,
            thread_cache: RwLock::new(HashMap::new()),
            users: RwLock::new(Vec::new()),
        }
    }
}
```

- [ ] **Step 2: Wire config loading into main.rs**

Update `main.rs` to read `.gitim/config.yaml`, validate with `validate_config`, create `AppState`, and scan `users/` directory to populate user list.

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p gitim-daemon`
Expected: compiles

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-daemon/
git commit -m "feat(daemon): add shared state with config loading and user scanning"
```

---

### Task I.2: Implement Request Handlers

**Files:**
- Create: `crates/gitim-daemon/src/handlers.rs`
- Modify: `crates/gitim-daemon/src/server.rs`

- [ ] **Step 1: Implement `send` handler**

Wire: compliance validate → format message → append to `.thread` file → git commit.

- [ ] **Step 2: Implement `read` handler**

Wire: parse thread (from cache or file) → filter by limit/since → return messages as JSON.

- [ ] **Step 3: Implement `channels`, `users`, `thread` handlers**

- `channels`: scan `channels/` for `.meta.json` files, return list.
- `users`: scan `users/` for `.meta.json` files, return list.
- `thread`: parse thread → follow P chain from given line number → return full thread.

- [ ] **Step 4: Add DM support to `send` and `read` handlers**

When channel argument uses `dm:handler1,handler2` format:
- Generate DM filename via `dm_filename()`
- Route to `dm/` directory instead of `channels/`

- [ ] **Step 5: Wire handlers into server with shared state**

Update `server.rs` and `http.rs` to pass `SharedState` to handlers.

- [ ] **Step 6: Commit**

```bash
git add crates/gitim-daemon/
git commit -m "feat(daemon): implement all request handlers — send, read, channels, users, thread, DM"
```

---

### Task I.3: Wire Sync & Watcher into Daemon

**Files:**
- Modify: `crates/gitim-daemon/src/main.rs`

- [ ] **Step 1: Start sync loop from config**

Read `config.daemon.sync_interval`, start `gitim_sync::sync_loop::start_sync_loop` as tokio task.

- [ ] **Step 2: Start file watcher with cache invalidation**

Start `gitim_sync::watcher::watch_repo`, consume `FileEvent`s to invalidate `thread_cache` entries.

- [ ] **Step 3: Run read-path integrity check after sync**

After each git pull, scan changed `.thread` files and run `check_thread_integrity`. Log warnings for any issues found.

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-daemon/
git commit -m "feat(daemon): wire sync loop, file watcher, and read-path integrity checks"
```

---

### Task I.4: End-to-End Test

**Files:**
- Create: `tests/e2e_test.sh`

- [ ] **Step 1: Write end-to-end test script**

```bash
#!/bin/bash
set -e

TESTDIR=$(mktemp -d)
cd "$TESTDIR"

# Initialize
git init
gitim init
echo '{"display_name":"Test","role":"dev","introduction":"hi"}' > users/tester.meta.json
git add -A && git commit -m "init"

# Start daemon
cargo run -p gitim-daemon &
DAEMON_PID=$!
sleep 1

# Test status
gitim status

# Test send + read
gitim send -c general -a tester "hello world"
gitim read -c general

# Test DM
gitim send --dm tester,tester2 -a tester "dm test"

# Cleanup
kill $DAEMON_PID
rm -rf "$TESTDIR"
echo "E2E test passed"
```

- [ ] **Step 2: Run test**

Run: `bash tests/e2e_test.sh`
Expected: "E2E test passed"

- [ ] **Step 3: Commit**

```bash
git add tests/
git commit -m "test: add end-to-end integration test"
```

---

### Task I.5: Add DM CLI Commands

**Files:**
- Create: `cli/src/commands/dm.ts`
- Modify: `cli/src/index.ts`

- [ ] **Step 1: Add DM send and read subcommands**

`gitim dm send <handler> -a <author> "message"` — sends DM
`gitim dm read <handler>` — reads DM thread
`gitim dm list` — lists all DM conversations

- [ ] **Step 2: Verify it compiles**

Run: `cd cli && npx tsc --noEmit`
Expected: no errors

- [ ] **Step 3: Commit**

```bash
git add cli/src/
git commit -m "feat(cli): add DM commands — send, read, list"
```

---

## Summary: Parallel Execution Map

| Stream | Tasks | Dependencies | Can start after |
|--------|-------|-------------|-----------------|
| Phase 0 | 0.1, 0.2, 0.3 | None | Immediately |
| Stream A | A.1–A.5 | Phase 0 | Phase 0 complete |
| Stream A+ | A.6, A.7 | Phase 0 | Phase 0 complete (parallel with A.1–A.5) |
| Stream B | B.1, B.2, B.3 | Phase 0 (types only) | Phase 0 complete |
| Stream C | C.1, C.2, C.3 | Phase 0 (types only) | Phase 0 complete |
| Stream C+ | C.4 | A.5 (needs formatter) | Stream A.5 complete |
| Stream D | D.1, D.2, D.3 | Phase 0 | Phase 0 complete |
| Integration | I.1–I.5 | A + B + C + D all complete | All streams complete |
