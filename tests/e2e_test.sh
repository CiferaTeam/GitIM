#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DAEMON_BIN="$REPO_ROOT/target/debug/gitim-daemon"

# Build daemon
echo "=== Building daemon ==="
cargo build --bin gitim-daemon --manifest-path "$REPO_ROOT/Cargo.toml"

# Create temp repo
TMPDIR=$(mktemp -d)
trap 'kill $(cat "$TMPDIR/.gitim/run/gitim.pid" 2>/dev/null) 2>/dev/null || true; rm -rf "$TMPDIR"' EXIT

cd "$TMPDIR"
git init
git config user.email "test@test.com"
git config user.name "Test"

# Setup GitIM structure
mkdir -p .gitim users channels
echo "version: 1" > .gitim/config.yaml

# Write me.json (simulating onboard)
cat > .gitim/me.json <<'MEJSON'
{"handler":"tester","endpoint":"github","inferred_from":"test","inferred_at":"20260317T120000Z"}
MEJSON

# Create user
cat > users/tester.meta.json <<'USERMETA'
{"display_name":"Tester","role":"dev","introduction":"hi"}
USERMETA

# Create channel
cat > channels/general.meta.json <<'CHANMETA'
{"display_name":"General","created_by":"tester","created_at":"20260317T120000Z","introduction":"test channel"}
CHANMETA
touch channels/general.thread

# .gitignore
echo -e ".gitim/run/\n.gitim/me.json" > .gitignore

git add -A && git commit -m "init" --quiet

# Start daemon
echo "=== Starting daemon ==="
PATH="$(dirname "$DAEMON_BIN"):$PATH"
gitim-daemon &
DAEMON_PID=$!

# Wait for socket
SOCK="$TMPDIR/.gitim/run/gitim.sock"
for i in $(seq 1 50); do
  [ -S "$SOCK" ] && break
  sleep 0.1
done
[ -S "$SOCK" ] || { echo "FAIL: daemon socket not ready"; exit 1; }

echo "=== Running tests ==="

# Test: status
RES=$(echo '{"method":"status"}' | nc -U "$SOCK")
echo "$RES" | grep -q '"ok":true' || { echo "FAIL: status"; exit 1; }
echo "PASS: status"

# Test: send WITHOUT author (should use me.json identity)
RES=$(echo '{"method":"send","channel":"general","body":"hello no author"}' | nc -U "$SOCK")
echo "$RES" | grep -q '"ok":true' || { echo "FAIL: send without author ($RES)"; exit 1; }
echo "PASS: send without author"

# Test: send WITH explicit author
RES=$(echo '{"method":"send","channel":"general","body":"hello with author","author":"tester"}' | nc -U "$SOCK")
echo "$RES" | grep -q '"ok":true' || { echo "FAIL: send with author ($RES)"; exit 1; }
echo "PASS: send with author"

# Test: read — verify both messages have @tester
RES=$(echo '{"method":"read","channel":"general"}' | nc -U "$SOCK")
echo "$RES" | grep -q '"ok":true' || { echo "FAIL: read ($RES)"; exit 1; }
echo "$RES" | grep -q '"author":"tester"' || { echo "FAIL: read author check ($RES)"; exit 1; }
echo "PASS: read with identity"

# Test: list channels
RES=$(echo '{"method":"channels"}' | nc -U "$SOCK")
echo "$RES" | grep -q 'general' || { echo "FAIL: channels ($RES)"; exit 1; }
echo "PASS: channels"

# Test: list users
RES=$(echo '{"method":"users"}' | nc -U "$SOCK")
echo "$RES" | grep -q 'tester' || { echo "FAIL: users ($RES)"; exit 1; }
echo "PASS: users"

# Test: register_user (new user)
RES=$(echo '{"method":"register_user","handler":"newbie","display_name":"New User"}' | nc -U "$SOCK")
echo "$RES" | grep -q '"ok":true' || { echo "FAIL: register_user ($RES)"; exit 1; }
[ -f "$TMPDIR/users/newbie.meta.json" ] || { echo "FAIL: newbie meta not created"; exit 1; }
echo "PASS: register_user"

# Test: register_user (existing user, should succeed with exists=true)
RES=$(echo '{"method":"register_user","handler":"tester","display_name":"Tester"}' | nc -U "$SOCK")
echo "$RES" | grep -q '"exists":true' || { echo "FAIL: register_user existing ($RES)"; exit 1; }
echo "PASS: register_user existing"

# === Test: Poll (cursor-pull) ===
echo "=== Test: Poll ==="

# Poll with no cursor — should get commit_id
POLL1=$(echo '{"method":"poll"}' | nc -U "$SOCK" -w 2)
echo "Poll (no cursor): $POLL1"
COMMIT_ID=$(echo "$POLL1" | jq -r '.data.commit_id')
if [ -z "$COMMIT_ID" ] || [ "$COMMIT_ID" = "null" ]; then
  echo "FAIL: poll did not return commit_id"
  exit 1
fi
echo "Got cursor: $COMMIT_ID"

# Send a message
SEND_RESULT=$(echo '{"method":"send","channel":"general","body":"poll test message"}' | nc -U "$SOCK" -w 2)
echo "Send: $SEND_RESULT"

# Small delay to let commit settle
sleep 1

# Poll with cursor — should see the new message
POLL2=$(echo "{\"method\":\"poll\",\"since\":\"$COMMIT_ID\"}" | nc -U "$SOCK" -w 2)
echo "Poll (with cursor): $POLL2"
HAS_CHANGES=$(echo "$POLL2" | jq '.data.changes | length')
NEW_COMMIT=$(echo "$POLL2" | jq -r '.data.commit_id')

if [ "$HAS_CHANGES" -gt 0 ]; then
  echo "PASS: poll detected $HAS_CHANGES channel(s) with changes"
else
  echo "FAIL: poll did not detect changes"
  exit 1
fi

# Poll with latest cursor — should be empty
POLL3=$(echo "{\"method\":\"poll\",\"since\":\"$NEW_COMMIT\"}" | nc -U "$SOCK" -w 2)
echo "Poll (latest cursor): $POLL3"
NO_CHANGES=$(echo "$POLL3" | jq '.data.changes | length')
if [ "$NO_CHANGES" -eq 0 ]; then
  echo "PASS: poll with latest cursor returns empty changes"
else
  echo "FAIL: poll returned unexpected changes with latest cursor"
  exit 1
fi

# Test: stop
RES=$(echo '{"method":"stop"}' | nc -U "$SOCK")
echo "$RES" | grep -q '"stopping"' || { echo "FAIL: stop ($RES)"; exit 1; }
sleep 0.5
kill -0 $DAEMON_PID 2>/dev/null && { echo "FAIL: daemon still running after stop"; exit 1; }
echo "PASS: stop"

echo ""
echo "=== All tests passed ==="
