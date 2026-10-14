# xterm 5.5.0 recovery lifecycle patch

The exact `@xterm/xterm@5.5.0` dependency is patched by pnpm; the patch hash is
part of `pnpm-lock.yaml`. There is no runtime monkey-patching or delayed terminal
disposal. Vite and release packaging consume the patched `lib/xterm.js` entry.
Upstream MIT license/copyright files are retained. Corresponding TypeScript
source changes are included for review; the original minified source-map comment
is removed because its offsets no longer describe the patched entry point.

## Why

- `Viewport.reset()` schedules an animation-frame callback which can run after
  disposal. `syncScrollArea()` then dereferences a destroyed renderer. The guard
  exits before touching disposed resources, including callbacks queued by open.
- `disableStdin` ordinarily checks only at delivery time. An IME composition or
  keyCode 229 textarea timer begun while input was blocked could run after input
  became writable. A monotonic core input generation invalidates those deferred
  operations across stdin transitions, terminal reset and disposal. Normal new
  compositions after recovery continue to work. Native textarea editability and
  focus are not changed, avoiding mobile keyboard side effects.

## Reproduce and verify

Use the repository's pinned pnpm 10.17.1:

```sh
pnpm patch @xterm/xterm@5.5.0 --ignore-existing --edit-dir /tmp/ergent-xterm-patch
python3 scripts/patch-xterm-5.5.0.py /tmp/ergent-xterm-patch
pnpm patch-commit /tmp/ergent-xterm-patch --patches-dir patches
pnpm install --frozen-lockfile
pnpm exec playwright test tests/e2e/xterm-lifecycle.spec.ts
pnpm build
```

The generation script checks the upstream version and each exact replacement
before writing. The regression tests load the actual bundled entry point in an
isolated VM and drive its real Viewport, CompositionHelper and Terminal/CoreService
with controlled timers. Private webpack module IDs are test-only and pinned to
this version. `ERGENT_XTERM_BUNDLE=/path/to/unpatched/lib/xterm.js` runs the same
regressions against an unpatched baseline: eight fail, while the already-safe
post-disposal input-emitter case passes. All nine pass with the patch.

On xterm upgrades, reassess both fixes against upstream, regenerate or remove the
patch intentionally, update the test-only module IDs if needed, and rerun the
full real-browser suite. Do not bypass a patch-hash mismatch or suppress page
errors. This coverage is not a claim of physical mobile keyboard certification.
