#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"

# ---------- 检测目标 bin 目录 ----------
detect_bin_dir() {
  # 优先级：/usr/local/bin > /opt/homebrew/bin (macOS) > ~/.local/bin
  if [ -d /usr/local/bin ] && [ -w /usr/local/bin ]; then
    echo "/usr/local/bin"
  elif [ -d /opt/homebrew/bin ] && [ -w /opt/homebrew/bin ]; then
    echo "/opt/homebrew/bin"
  else
    local d="$HOME/.local/bin"
    mkdir -p "$d"
    echo "$d"
  fi
}

BIN_DIR="${INSTALL_DIR:-$(detect_bin_dir)}"

echo "==> 目标安装目录: $BIN_DIR"

# ---------- gitim-daemon (Rust) ----------
echo "==> 构建并安装 gitim-daemon ..."
cargo install --path "$ROOT/crates/gitim-daemon"
echo "    gitim-daemon -> ~/.cargo/bin/gitim-daemon"

# ---------- gitim CLI (Node) ----------
echo "==> 构建 gitim CLI ..."
cd "$ROOT/cli"
npm install --ignore-scripts 2>/dev/null
npm run build

# 创建 wrapper 脚本，避免依赖 npm link
CLI_ENTRY="$ROOT/cli/dist/index.js"
if [ ! -f "$CLI_ENTRY" ]; then
  echo "错误: CLI 入口 $CLI_ENTRY 不存在"
  exit 1
fi

cat > "$BIN_DIR/gitim" <<WRAPPER
#!/usr/bin/env node
import("$CLI_ENTRY");
WRAPPER
chmod +x "$BIN_DIR/gitim"
echo "    gitim        -> $BIN_DIR/gitim"

# ---------- 验证 ----------
echo ""
echo "==> 安装完成"
echo "    gitim-daemon -> ~/.cargo/bin/gitim-daemon"
echo "    gitim        -> $BIN_DIR/gitim"

# 检查 PATH
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) echo ""
     echo "注意: $BIN_DIR 不在 PATH 中，请添加:"
     echo "  export PATH=\"$BIN_DIR:\$PATH\""
     ;;
esac
