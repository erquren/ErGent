#!/bin/sh
# Probe the configured listener locally, retaining certificate/hostname checks.
set -eu
listen=${ERGENT_LISTEN:-0.0.0.0:7777}
port=${listen##*:}
host=${listen%:*}
case "$host" in
  0.0.0.0) host=127.0.0.1 ;;
  '[::]') host='[::1]' ;;
esac
if [ "${ERGENT_TLS_MODE:-direct}" = direct ]; then
  test -n "${ERGENT_TLS_CERT:-}" && test -n "${ERGENT_ORIGIN:-}"
  exec curl --fail --silent --show-error --max-time 3 --noproxy '*' \
    --cacert "$ERGENT_TLS_CERT" --connect-to "::$host:$port" \
    "$ERGENT_ORIGIN/health/ready"
else
  exec curl --fail --silent --show-error --max-time 3 --noproxy '*' \
    "http://$host:$port/health/ready"
fi
