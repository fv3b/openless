#!/bin/bash
set -euo pipefail

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

cat > "$TMP/probe.swift" <<'EOF'
import Foundation
import Security

let args = CommandLine.arguments
let account = args.count > 1 ? args[1] : "credentials.v1"

let query: [String: Any] = [
  kSecClass as String: kSecClassGenericPassword,
  kSecAttrService as String: "com.openless.app",
  kSecAttrAccount as String: account,
  kSecReturnData as String: true,
]
var item: CFTypeRef?
let status = SecItemCopyMatching(query as CFDictionary, &item)
if status == errSecSuccess, let data = item as? Data {
  print("SILENT_OK bytes=\(data.count)")
  exit(0)
} else {
  print("WOULD_PROMPT status=\(status)")
  exit(1)
}
EOF

swiftc -O -o "$TMP/probe" "$TMP/probe.swift" 2>/dev/null
codesign --force --sign "OpenLess Dev Signing" -i com.openless.app "$TMP/probe"

ACCOUNT="${1:-credentials.v1}"
if "$TMP/probe" "$ACCOUNT"; then
  echo "钥匙串条目 [$ACCOUNT] 对 OpenLess Dev Signing 签名已永久放行（不再弹窗、不再要密码）"
else
  echo "钥匙串条目 [$ACCOUNT] 尚未放行当前签名，运行 App 后在弹窗里点「始终允许」（不是「允许」）"
fi
