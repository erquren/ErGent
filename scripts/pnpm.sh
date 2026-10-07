#!/bin/sh
set -eu
PROJECT_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
if [ -x "$PROJECT_ROOT/.tools/node/node_modules/.bin/pnpm" ]; then
  export PATH="$PROJECT_ROOT/.tools/node/node_modules/.bin:$PATH"
  exec "$PROJECT_ROOT/.tools/node/node_modules/.bin/pnpm" "$@"
fi
exec pnpm "$@"
