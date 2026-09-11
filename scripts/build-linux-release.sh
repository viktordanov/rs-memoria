#!/usr/bin/env bash
# Build one Linux release executable inside a pinned immutable builder image,
# then package it with scripts/package-linux-release.sh.
#
# The builder image comes from scripts/linux-builders.json by digest. A tag is
# never used, because a tag can move.
#
# The build is native: the container platform matches the requested target, and
# the host architecture must match it too. A cross-build would not prove that
# the produced executable runs.
#
# Usage:
#   scripts/build-linux-release.sh --target x86_64-unknown-linux-gnu --output DIR
#   scripts/build-linux-release.sh --target aarch64-unknown-linux-gnu --output DIR
set -euo pipefail

target=""
output=""
version=""
manifest=""

while [ "$#" -gt 0 ]; do
  case "$1" in
    --target) target="${2:?--target needs a value}"; shift 2 ;;
    --output) output="${2:?--output needs a value}"; shift 2 ;;
    --version) version="${2:?--version needs a value}"; shift 2 ;;
    --manifest) manifest="${2:?--manifest needs a value}"; shift 2 ;;
    -h|--help) sed -n '2,14p' "$0"; exit 0 ;;
    *) echo "build-linux-release: unknown argument $1" >&2; exit 2 ;;
  esac
done

if [ -z "$target" ] || [ -z "$output" ]; then
  echo "build-linux-release: --target and --output are required" >&2
  exit 2
fi

repository_root="$(cd "$(dirname "$0")/.." && pwd)"
manifest="${manifest:-$repository_root/scripts/linux-builders.json}"

if [ ! -f "$manifest" ]; then
  echo "build-linux-release: builder manifest $manifest is missing" >&2
  exit 2
fi

read_manifest() {
  python3 - "$manifest" "$target" "$1" <<'PY'
import json, sys
manifest, target, field = sys.argv[1], sys.argv[2], sys.argv[3]
with open(manifest, "r", encoding="utf-8") as handle:
    data = json.load(handle)
builders = data.get("builders", {})
if target not in builders:
    sys.exit(f"no builder is pinned for {target}")
if field in ("image", "toolchain"):
    print(data[field])
else:
    print(builders[target][field])
PY
}

image="$(read_manifest image)"
toolchain="$(read_manifest toolchain)"
digest="$(read_manifest digest)"
platform="$(read_manifest platform)"

if [ -z "$version" ]; then
  version="$(python3 - "$repository_root/Cargo.toml" <<'PY'
import re, sys
text = open(sys.argv[1], "r", encoding="utf-8").read()
section = text.split("[workspace.package]", 1)[1]
print(re.search(r'^version = "([^"]+)"', section, re.M).group(1))
PY
)"
fi

# The produced executable must run on this machine, so the host architecture
# has to be the target architecture.
host_machine="$(uname -m)"
case "$target" in
  x86_64-unknown-linux-gnu) expected_machine="x86_64" ;;
  aarch64-unknown-linux-gnu) expected_machine="aarch64" ;;
  *) echo "build-linux-release: unsupported target $target" >&2; exit 2 ;;
esac
if [ "$host_machine" != "$expected_machine" ]; then
  echo "build-linux-release: $target needs a native $expected_machine host; this host is $host_machine" >&2
  exit 2
fi

SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git -C "$repository_root" show -s --format=%ct HEAD)}"
export SOURCE_DATE_EPOCH
source_commit="$(git -C "$repository_root" rev-parse HEAD 2>/dev/null || echo unknown)"

mkdir -p "$output"
output="$(cd "$output" && pwd)"

# Each invocation builds in a fresh directory, and the container always sees
# the same absolute paths, so the build path cannot vary between runs.
work="$(mktemp -d "${output}/.build-XXXXXX")"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/src" "$work/target" "$work/cargo"
# The working tree is the build input, exactly as a clean tag checkout would
# be. Git metadata and any local build directory stay out of the container.
#
# An output directory inside the repository would otherwise be copied while
# this build writes into it, and tar would stop with "file changed as we read
# it". Exclude it explicitly when it sits under the repository root.
copy_excludes=(--exclude=./.git --exclude=./target --exclude=./.github)
case "$output" in
  "$repository_root"/*)
    copy_excludes+=("--exclude=./${output#"$repository_root"/}")
    ;;
  "$repository_root")
    echo "build-linux-release: --output must not be the repository root" >&2
    exit 2
    ;;
esac
tar -C "$repository_root" "${copy_excludes[@]}" -cf - . | tar -x -C "$work/src"

echo "build-linux-release: target ${target}"
echo "build-linux-release: image ${image}@${digest}"
echo "build-linux-release: toolchain ${toolchain}"
echo "build-linux-release: source ${source_commit}"
echo "build-linux-release: SOURCE_DATE_EPOCH ${SOURCE_DATE_EPOCH}"

# The container runs as the invoking account, so every produced file belongs
# to that account and the build never writes into the image.
docker run --rm \
  --platform "$platform" \
  --user "$(id -u):$(id -g)" \
  --env SOURCE_DATE_EPOCH="$SOURCE_DATE_EPOCH" \
  --env RUSTUP_TOOLCHAIN="$toolchain" \
  --env CARGO_HOME=/build/cargo \
  --env CARGO_TARGET_DIR=/build/target \
  --env CARGO_INCREMENTAL=0 \
  --env RUSTFLAGS="--remap-path-prefix=/build/src=/memoria --remap-path-prefix=/build/cargo=/cargo" \
  --volume "$work:/build" \
  --workdir /build/src \
  "${image}@${digest}" \
  sh -euc "
    rustc --version
    cargo --version
    test \"\$(rustc --version | cut -d' ' -f2)\" = '${toolchain}'
    # The build is native, so the target is the image's own default target.
    rustup target list --installed | grep -qx '${target}'
    # Cargo.lock pins every dependency version. The fetch is the one step
    # that needs the network; the build itself then runs offline.
    cargo fetch --locked
    cargo build --release --locked --offline --target '${target}' --bin memoria
  "

binary="$work/target/${target}/release/memoria"
if [ ! -x "$binary" ]; then
  echo "build-linux-release: the builder produced no executable" >&2
  exit 1
fi

install -m 755 "$binary" "${output}/memoria"
"$repository_root/scripts/package-linux-release.sh" \
  --version "$version" --target "$target" \
  --binary "${output}/memoria" --output "$output"

{
  echo "source_commit: ${source_commit}"
  echo "target: ${target}"
  echo "version: ${version}"
  echo "builder_image: ${image}@${digest}"
  echo "builder_platform: ${platform}"
  echo "toolchain: ${toolchain}"
  echo "source_date_epoch: ${SOURCE_DATE_EPOCH}"
  echo "host_machine: ${host_machine}"
  echo "binary_sha256: $(sha256sum "${output}/memoria" | cut -d' ' -f1)"
  echo "archive_sha256: $(cut -d' ' -f1 < "${output}/memoria-${version}-${target}.tar.gz.sha256")"
} > "${output}/provenance.txt"

cat "${output}/provenance.txt"
