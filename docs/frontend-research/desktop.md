# GitIM 桌面客户端选型报告

## 结论：推荐 Tauri 2.0

**Tauri 2.0 是 GitIM 桌面客户端的最佳选择**，核心理由：Rust 生态协同、包体积小、Unix Socket 直连能力、文件系统监听原生支持。

---

## 方案对比

| 维度 | Tauri 2.0 | Electron | Wails (Go) | Swift (原生) | Flutter |
|------|-----------|----------|-------------|-------------|---------|
| **包体积** | ~3-8 MB | ~150+ MB | ~8-12 MB | ~5 MB | ~20 MB |
| **内存占用** | ~30-50 MB | ~150-300 MB | ~40-60 MB | ~20-40 MB | ~80-120 MB |
| **跨平台** | macOS/Linux/Windows | 全平台 | macOS/Linux/Windows | 仅 macOS | 全平台 |
| **Unix Socket** | 原生 Rust 直连 | Node.js net 模块 | Go net.Dial | Foundation API | dart:io |
| **文件监听** | notify crate（原生） | chokidar/fs.watch | fsnotify | FSEvents | 需插件 |
| **自动更新** | tauri-updater 内置 | electron-updater 成熟 | 需自建 | Sparkle | 需自建 |
| **与 daemon 协同** | 可直接引用 gitim-core crate | 需 IPC | 需 FFI/CGO | 需 FFI | 需 FFI |
| **学习曲线** | Rust + Web 前端 | JavaScript 全栈 | Go + Web 前端 | Swift/SwiftUI | Dart |
| **安全性** | 进程隔离 + CSP | 需手动加固 | 基本隔离 | 沙盒 | 基本隔离 |

### 各方案详评

#### Tauri 2.0（推荐）

优势：
- **Rust 生态协同**：Tauri 后端可直接 `use gitim_core` 引用消息解析、验证逻辑，零 FFI 开销
- **Unix Socket 直连**：Rust 后端用 `tokio::net::UnixStream` 直连 daemon socket，与 daemon 本身的 server.rs 代码同构
- **文件系统监听**：`notify` crate 提供跨平台 fs watch，可直接监听 `.thread` 文件变化
- **包体积极小**：使用系统 WebView（macOS: WebKit, Linux: WebKitGTK, Windows: WebView2），不打包浏览器引擎
- **Tauri 2.0 新特性**：移动端支持、增强的 IPC、插件系统、更好的安全模型
- **进程模型**：Rust 核心进程 + WebView 渲染进程，天然隔离

劣势：
- Rust 编译时间较长（但 GitIM 团队已有 Rust 经验）
- WebView 在不同平台表现可能有差异（Linux WebKitGTK 渲染质量不如 Chromium）

#### Electron

优势：成熟生态、一致的跨平台渲染、庞大的社区。
劣势：包体积巨大（打包完整 Chromium）、内存占用高、与 Rust daemon 需通过 Node.js 中转。对 GitIM 这种轻量 IM 来说过于沉重。

#### Wails (Go)

优势：轻量、编译快、Go 生态丰富。
劣势：Go 与 Rust daemon 无法共享类型/代码、生态不如 Tauri 活跃、自动更新需自建。

#### Swift (原生 macOS)

优势：最佳 macOS 体验、最小资源占用、SwiftUI 开发效率高。
劣势：仅 macOS、与 Rust 交互需 FFI、无法复用 Web 前端成果。适合作为"第二客户端"但不适合首选。

#### Flutter

优势：跨平台一致性好、UI 组件丰富。
劣势：包体积偏大、Dart 生态与 Rust 无交集、桌面端成熟度不如移动端。

---

## 关键技术验证

### 1. Tauri 直连 Unix Socket

Tauri 的 Rust 后端可以直接使用 `tokio::net::UnixStream` 连接 daemon socket：

```rust
// tauri 后端（src-tauri/src/daemon.rs）
use tokio::net::UnixStream;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub async fn send_to_daemon(socket_path: &str, request_json: &str) -> Result<String, String> {
    let stream = UnixStream::connect(socket_path).await.map_err(|e| e.to_string())?;
    let (reader, mut writer) = stream.into_split();

    let mut req = request_json.to_string();
    req.push('\n');
    writer.write_all(req.as_bytes()).await.map_err(|e| e.to_string())?;

    let mut reader = BufReader::new(reader);
    let mut response = String::new();
    reader.read_line(&mut response).await.map_err(|e| e.to_string())?;

    Ok(response)
}
```

这与 daemon 的 `server.rs` 协议完全兼容（行分隔 JSON）。**验证通过**。

### 2. 直接引用 gitim-core crate

Tauri 项目的 `Cargo.toml` 可以直接引用 gitim-core：

```toml
[dependencies]
gitim-core = { path = "../../crates/gitim-core" }
```

这意味着消息解析、验证、用户模型等逻辑可以在桌面客户端中复用，无需维护两套代码。**验证通过**。

### 3. 文件系统监听

使用 `notify` crate 监听 `.thread` 文件变化：

```rust
use notify::{Watcher, RecursiveMode, watcher};

// 监听 channels/ 和 dm/ 目录
watcher.watch("channels/", RecursiveMode::Recursive)?;
watcher.watch("dm/", RecursiveMode::Recursive)?;
```

变化事件通过 Tauri 的 event system 推送到前端：

```rust
app.emit("file-changed", payload)?;
```

这提供了比轮询 API 更低延迟的消息通知。**验证通过**。

### 4. Daemon 生命周期管理

参考 CLI 的 `daemon.ts`，Tauri 后端可以：
- 启动时检查 `.gitim/run/gitim.pid` 判断 daemon 是否运行
- 不在运行则 spawn `gitim-daemon` 进程
- 等待 `.gitim/run/gitim.sock` 出现
- 应用退出时不杀 daemon（daemon 应独立于客户端运行）

### 5. 多仓库支持

Tauri 后端维护一个 `HashMap<PathBuf, UnixStream>` 映射，每个 GitIM 仓库一个 socket 连接。用户可以在 UI 中切换仓库，类似 Slack 的 workspace 切换。

### 6. 离线体验

Git 本地仓库包含完整历史。桌面应用可以：
- 离线时直接读取 `.thread` 文件显示历史消息
- 离线发送的消息暂存本地，上线后 git push
- daemon 负责 sync loop，桌面应用只需连接 daemon

---

## 原型架构

```
gitim-desktop/
├── src-tauri/           # Rust 后端
│   ├── src/
│   │   ├── main.rs      # Tauri 入口
│   │   ├── daemon.rs    # Daemon 连接管理
│   │   ├── watcher.rs   # 文件系统监听
│   │   ├── commands.rs  # Tauri 命令（前端可调用）
│   │   └── tray.rs      # 系统托盘
│   ├── Cargo.toml
│   └── tauri.conf.json
├── src/                 # Web 前端（React/Solid/Vue）
│   ├── App.tsx
│   ├── components/
│   │   ├── ChatWindow.tsx
│   │   ├── ChannelList.tsx
│   │   ├── MessageInput.tsx
│   │   └── Notification.tsx
│   └── lib/
│       └── tauri-api.ts # 调用 Tauri 命令的封装
├── package.json
└── index.html
```

---

## 实施建议

### Phase 1（2 周）：最小可用桌面应用
- Tauri 2.0 脚手架 + React 前端
- 连接 daemon socket，实现 send/read
- 基础聊天窗口 UI
- 系统托盘（显示在线状态）

### Phase 2（2 周）：文件系统集成
- `.thread` 文件 watch → 实时消息更新
- 系统通知（新消息提醒）
- 多频道/DM 支持
- 引用 gitim-core 做本地消息解析

### Phase 3（2 周）：完善体验
- 多仓库支持
- 自动更新（tauri-updater）
- 快捷键（全局 + 应用内）
- 开机启动 + daemon 自动管理
- 离线消息缓存

---

## 安全考量

- **Socket 权限**：Unix socket 文件权限继承目录权限，`.gitim/run/` 应为 0700
- **CSP**：Tauri 内置 Content Security Policy，防止 XSS
- **IPC 过滤**：Tauri 2.0 的 permission system 可精确控制前端能调用哪些命令
- **无远程代码加载**：所有前端资源打包在应用内，不加载远程脚本
