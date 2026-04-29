#!/bin/sh
IFS= read -r line

case "$line" in
  *'"type":"prompt"'*)
    printf '%s\n' '{"type":"response","command":"prompt","success":false,"error":"Cannot read properties of undefined (reading '\''startsWith'\'')"}'
    exit 0
    ;;
  *)
    printf 'unexpected stdin: %s\n' "$line" >&2
    exit 2
    ;;
esac
