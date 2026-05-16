#!/bin/sh
# Fixture for kimi preflight timeout test: stalls for longer than any
# reasonable test timeout. The --version short-circuit lets the
# version-probe phase pass quickly so the test exercises the inner
# ACP-handshake timeout, not the version-call timeout.
if [ "$1" = "--version" ]; then
  echo "kimi 0.99.0-mock"
  exit 0
fi
sleep 10
