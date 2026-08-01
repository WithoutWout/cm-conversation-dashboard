#!/usr/bin/env bash
#
# Builds the static, 100% client-side web app into dist/.
# Upload the contents of dist/ to any web host — no server code, no headers.
#
#   ./build-web.sh            release build
#   ./build-web.sh --dev      faster build, much larger .wasm
#   ./build-web.sh --serve    build, then serve dist-web/ on :8777 to try it
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# NOT dist/ — that is electron-builder's output directory and already holds
# the desktop installers.
DIST="$ROOT/dist-web"
PROFILE="--release"
SERVE=0
for arg in "$@"; do
  case "$arg" in
    --dev) PROFILE="--dev" ;;
    --serve) SERVE=1 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

# The libsqlite3-sys shim MUST be applied here rather than in Cargo.toml:
# [patch] is not target-conditional, so a manifest patch would also replace the
# real bundled SQLite for the native desktop build and break every native test.
# Passing it per-invocation is what makes it wasm-only. The path is resolved
# against the current directory, so it is made absolute rather than relative.
PATCH="patch.crates-io.libsqlite3-sys.path=\"$ROOT/vendor/libsqlite3-sys-wasm\""

command -v wasm-pack >/dev/null || {
  echo "wasm-pack not found. Install it with: cargo install wasm-pack" >&2
  exit 1
}

# Apple clang cannot emit wasm32; .cargo/config.toml points cc at Homebrew LLVM.
# Warn early rather than failing deep inside a C build.
if [[ "$(uname -s)" == "Darwin" && -z "${CC_wasm32_unknown_unknown:-}" ]]; then
  [[ -x /opt/homebrew/opt/llvm/bin/clang ]] || {
    echo "No wasm-capable C compiler found. Install one with: brew install llvm" >&2
    echo "(or export CC_wasm32_unknown_unknown / AR_wasm32_unknown_unknown yourself)" >&2
    exit 1
  }
fi

echo "==> building wasm ($PROFILE)"
wasm-pack build "$ROOT/src-tauri" \
  --target web \
  $PROFILE \
  --out-dir "$DIST/pkg" \
  --out-name cai_dashboard_lib \
  -- --config "$PATCH"

echo "==> assembling $DIST"
# Only the files the app actually loads. Deliberately explicit: a `cp -r` of
# frontend/ would also ship tests/ and any scratch file left lying around.
for f in index.html search-worker.js db-worker.js wasm-bridge.js \
         analytics-web.js analytics-fetch.js manifest.json sw.js cmwhitelogo.svg; do
  cp "$ROOT/frontend/$f" "$DIST/$f"
done
mkdir -p "$DIST/vendor" "$DIST/icons"
cp "$ROOT/frontend/vendor/vis-network.min.js" "$DIST/vendor/"

# Icons are committed under frontend/icons/ rather than generated here: they are
# static brand assets, so generating them per build would make the output depend
# on whichever image tool happens to be installed. Regenerate them with
# tools/make-icons.py if the source icon changes.
cp "$ROOT"/frontend/icons/*.png "$DIST/icons/"

# Drop what should never reach a web host: Finder metadata, and wasm-pack's
# npm-publishing files (package.json, .gitignore, the .d.ts type definitions).
# None are fetched by the app; they are only there because wasm-pack's default
# output is an npm package.
find "$DIST" -name '.DS_Store' -delete
rm -f "$DIST"/pkg/package.json "$DIST"/pkg/.gitignore "$DIST"/pkg/README.md
rm -f "$DIST"/pkg/*.d.ts

# Stamp the build id into the service worker. This is the whole cache strategy:
# a new id means a new cache name, so the previous build is dropped wholesale.
BUILD_ID="$(date -u +%Y%m%d-%H%M%S)-$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo nogit)"
if sed --version >/dev/null 2>&1; then
  sed -i "s/__BUILD_ID__/$BUILD_ID/" "$DIST/sw.js"          # GNU
else
  sed -i '' "s/__BUILD_ID__/$BUILD_ID/" "$DIST/sw.js"       # BSD/macOS
fi
grep -q "$BUILD_ID" "$DIST/sw.js" || { echo "failed to stamp BUILD_ID into sw.js" >&2; exit 1; }

echo "==> done: $BUILD_ID"
du -sh "$DIST" | awk '{print "    total: " $1}'
find "$DIST" -name '*.wasm' -exec du -h {} \; | awk '{print "    wasm:  " $1}'
echo "    upload the *contents* of dist-web/ to your web host"

if [[ "$SERVE" == "1" ]]; then
  echo "==> serving $DIST at http://localhost:8777 (Ctrl-C to stop)"
  echo "    a secure context is required for OPFS; localhost counts as one"
  exec python3 -m http.server 8777 --directory "$DIST"
fi
