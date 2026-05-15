#!/bin/sh
# Fixture for override tests: echoes argv + env var value to stderr, exits 1.
# Tests assert that the captured error string contains the model arg they
# passed and/or the env var value they injected.
#
# Exit code is 1 so the call lands in `preflight_*`'s non-zero-exit branch
# (which captures stderr into PreflightResult.error). We don't need a
# successful JSON parse path; we just need to observe what the parent
# process actually invoked.
echo "ARGV=$*" >&2
echo "MY_TEST_KEY=${MY_TEST_KEY:-<unset>}" >&2
exit 1
