#!/usr/bin/env bash
# Package one Linux release archive and its checksum sidecar.
#
# The archive layout is the contract the setup Action validates: one versioned
# directory that holds exactly LICENSE and memoria as regular files.
#
# Every archive field that a timestamp, a user name, or a file order could
# change is set explicitly, so two runs over the same inputs produce the same
# bytes.
#
# Usage:
#   scripts/package-linux-release.sh --version 0.5.0 \
#     --target x86_64-unknown-linux-gnu --binary path/to/memoria --output DIR
set -euo pipefail

version=""
target=""
binary=""
output=""

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version) version="${2:?--version needs a value}"; shift 2 ;;
    --target) target="${2:?--target needs a value}"; shift 2 ;;
    --binary) binary="${2:?--binary needs a value}"; shift 2 ;;
    --output) output="${2:?--output needs a value}"; shift 2 ;;
    -h|--help) sed -n '2,14p' "$0"; exit 0 ;;
    *) echo "package-linux-release: unknown argument $1" >&2; exit 2 ;;
  esac
done

for required in version target binary output; do
  if [ -z "${!required}" ]; then
    echo "package-linux-release: --${required} is required" >&2
    exit 2
  fi
done

case "$target" in
  x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu) ;;
  *) echo "package-linux-release: unsupported target $target" >&2; exit 2 ;;
esac

if [ ! -f "$binary" ]; then
  echo "package-linux-release: $binary is not a regular file" >&2
  exit 2
fi

repository_root="$(cd "$(dirname "$0")/.." && pwd)"
license="$repository_root/LICENSE"
if [ ! -f "$license" ]; then
  echo "package-linux-release: $license is missing" >&2
  exit 2
fi

# The commit timestamp is the one clock the archive uses. A build outside a
# Git checkout must supply SOURCE_DATE_EPOCH itself.
if [ -z "${SOURCE_DATE_EPOCH:-}" ]; then
  SOURCE_DATE_EPOCH="$(git -C "$repository_root" show -s --format=%ct HEAD)"
fi
export SOURCE_DATE_EPOCH

prefix="memoria-${version}-${target}"
asset="${prefix}.tar.gz"

mkdir -p "$output"
output="$(cd "$output" && pwd)"
staging="$(mktemp -d "${output}/.stage-XXXXXX")"
trap 'rm -rf "$staging"' EXIT

install -d -m 755 "$staging/$prefix"
install -m 755 "$binary" "$staging/$prefix/memoria"
install -m 644 "$license" "$staging/$prefix/LICENSE"

# --sort=name fixes the member order, --mtime fixes every timestamp, the owner
# options remove the build account, and `gzip -n` removes the compressor's own
# name and timestamp.
tar \
  --format=gnu \
  --sort=name \
  --mtime="@${SOURCE_DATE_EPOCH}" \
  --owner=0 \
  --group=0 \
  --numeric-owner \
  --mode='go-w' \
  -C "$staging" \
  -cf - "$prefix" \
  | gzip -n -9 > "${output}/${asset}"

( cd "$output" && sha256sum "$asset" > "${asset}.sha256" )

echo "package-linux-release: wrote ${output}/${asset}"
echo "package-linux-release: wrote ${output}/${asset}.sha256"
tar -tzf "${output}/${asset}"
