#!/usr/bin/env sh
set -eu

repo="${CXV_REPO:-alexzhang1030/codex-compaction-viewer}"
version="${CXV_VERSION:-latest}"
bin_dir="${CXV_BIN_DIR:-$HOME/.local/bin}"

os="$(uname -s)"
arch="$(uname -m)"

case "$os:$arch" in
  Darwin:arm64) asset="cxv-macos-aarch64.tar.gz" ;;
  Darwin:x86_64) asset="cxv-macos-x86_64.tar.gz" ;;
  Linux:x86_64) asset="cxv-linux-x86_64.tar.gz" ;;
  *)
    echo "unsupported platform: $os $arch" >&2
    exit 1
    ;;
esac

if [ "$version" = "latest" ]; then
  url="https://github.com/$repo/releases/latest/download/$asset"
  fallback_url="https://raw.githubusercontent.com/$repo/main/dist/$asset"
else
  url="https://github.com/$repo/releases/download/$version/$asset"
  fallback_url=""
fi

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

mkdir -p "$bin_dir"
if ! curl -fsSL "$url" -o "$tmp_dir/$asset" 2>/dev/null; then
  if [ -z "$fallback_url" ]; then
    exit 1
  fi
  curl -fsSL "$fallback_url" -o "$tmp_dir/$asset"
fi
tar -xzf "$tmp_dir/$asset" -C "$tmp_dir"
install -m 0755 "$tmp_dir/cxv" "$bin_dir/cxv"

echo "installed cxv to $bin_dir/cxv"
