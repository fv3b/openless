#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP_DIR="$ROOT/openless-all/app"
DST="/Applications/OpenLess.app"

PROFILE="--debug"
if [ "${1:-}" = "--release" ]; then
  PROFILE=""
fi
SRC="$APP_DIR/src-tauri/target/debug/bundle/macos/OpenLess.app"
if [ -z "$PROFILE" ]; then
  SRC="$APP_DIR/src-tauri/target/release/bundle/macos/OpenLess.app"
fi

cd "$APP_DIR"
npx tauri build $PROFILE -b app

SIG_INFO="$(codesign -dvvv "$SRC" 2>&1 || true)"
if ! echo "$SIG_INFO" | grep -q "Authority=OpenLess Dev Signing"; then
  echo "错误：构建产物不是 OpenLess Dev Signing 签名，拒绝安装。" >&2
  echo "adhoc 签名每次重编都会换身份，导致钥匙串/TCC 授权反复弹窗。" >&2
  exit 1
fi

osascript -e 'quit app "OpenLess"' >/dev/null 2>&1 || true
killall openless >/dev/null 2>&1 || true
sleep 1

rm -rf "$DST"
ditto "$SRC" "$DST"

codesign --force --sign "OpenLess Dev Signing" "$DST"

if codesign -dvv "$DST" 2>&1 | grep -q "flags=0x10000"; then
  echo "错误：构建产物仍带 hardened runtime，会导致钥匙串授权每次重编失效。" >&2
  exit 1
fi
codesign --verify --strict "$DST"

open "$DST"
echo "已构建、替换并启动 /Applications/OpenLess.app（签名身份 OpenLess Dev Signing）"
