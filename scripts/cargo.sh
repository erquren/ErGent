#!/bin/sh
set -eu
PROJECT_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
if [ -x "$PROJECT_ROOT/.tools/cargo/bin/cargo" ]; then
  export CARGO_HOME="$PROJECT_ROOT/.tools/cargo"
  export RUSTUP_HOME="$PROJECT_ROOT/.tools/rustup"
  export PATH="$CARGO_HOME/bin:$PATH"
fi
exec cargo "$@"
