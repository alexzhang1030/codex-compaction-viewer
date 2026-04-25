#!/usr/bin/env sh
set -eu

script="$(cd "$(dirname "$0")/.." && pwd)/scripts/install.sh"

if grep -q 'raw.githubusercontent.com.*/dist' "$script"; then
  echo "install.sh must not fall back to checked-in dist assets" >&2
  exit 1
fi

if ! grep -q 'releases/download' "$script"; then
  echo "install.sh must download versioned release assets" >&2
  exit 1
fi

if ! grep -q 'releases/latest/download' "$script"; then
  echo "install.sh must download latest release assets by default" >&2
  exit 1
fi
