#!/bin/bash
set -e

TESTDIR=$(mktemp -d)
SCRIPT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
DAEMON_BIN="$SCRIPT_DIR/target/debug/gitim-daemon"

echo "=== E2E Test ==="
echo "Test dir: $TESTDIR"
echo "Daemon: $DAEMON_BIN"

cd "$TESTDIR"

# Initialize git repo
git init
git config user.email "test@test.com"
git config user.name "Test"

# Create GitIM structure
mkdir -p .gitim users channels
echo 'version: 1' > .gitim/config.yaml
echo '.gitim/run/' > .gitignore
echo '{"display_name":"Tester","role":"dev","introduction":"hi"}' > users/tester.meta.json
echo '{"display_name":"General","created_by":"tester","created_at":"20250316T120000Z","introduction":"General channel"}' > channels/general.meta.json

git add -A && git commit -m "init"

# Start daemon
echo "Starting daemon..."
"$DAEMON_BIN" &
DAEMON_PID=$!
sleep 2

# Check daemon is running
if ! kill -0 $DAEMON_PID 2>/dev/null; then
    echo "FAIL: daemon did not start"
    rm -rf "$TESTDIR"
    exit 1
fi
echo "Daemon running (pid: $DAEMON_PID)"

# Test status via socket
echo '{"method":"status"}' | socat - UNIX-CONNECT:"$TESTDIR/.gitim/run/gitim.sock" | grep -q '"ok":true' && echo "PASS: status" || echo "FAIL: status"

# Test send
SEND_RESULT=$(echo '{"method":"send","channel":"general","body":"hello world","author":"tester"}' | socat - UNIX-CONNECT:"$TESTDIR/.gitim/run/gitim.sock")
echo "$SEND_RESULT" | grep -q '"ok":true' && echo "PASS: send" || echo "FAIL: send ($SEND_RESULT)"

# Test read
READ_RESULT=$(echo '{"method":"read","channel":"general"}' | socat - UNIX-CONNECT:"$TESTDIR/.gitim/run/gitim.sock")
echo "$READ_RESULT" | grep -q 'hello world' && echo "PASS: read" || echo "FAIL: read ($READ_RESULT)"

# Test list channels
CH_RESULT=$(echo '{"method":"channels"}' | socat - UNIX-CONNECT:"$TESTDIR/.gitim/run/gitim.sock")
echo "$CH_RESULT" | grep -q 'general' && echo "PASS: channels" || echo "FAIL: channels ($CH_RESULT)"

# Test list users
USR_RESULT=$(echo '{"method":"users"}' | socat - UNIX-CONNECT:"$TESTDIR/.gitim/run/gitim.sock")
echo "$USR_RESULT" | grep -q 'tester' && echo "PASS: users" || echo "FAIL: users ($USR_RESULT)"

# Cleanup
kill $DAEMON_PID 2>/dev/null || true
wait $DAEMON_PID 2>/dev/null || true
rm -rf "$TESTDIR"

echo "=== E2E Test Complete ==="
