#!/bin/sh
set -eu
cd -- "$(dirname -- "$0")"
exec ./ergent-server run --web "$PWD/web" "$@"
