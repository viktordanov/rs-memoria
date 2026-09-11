#!/usr/bin/env bash
# Export the consumer workflow that `memoria integrations github` generates.
#
# The harness builds a throwaway Git project outside this repository, invokes
# the real CLI preview and apply there, and copies the produced workflow to
# the requested output file. An independent parser can then examine those
# bytes.
#
# The harness never installs a workflow into this repository.
#
# Usage:
#   scripts/check-github-workflow-template.sh --binary ./target/release/memoria \
#     --output /tmp/generated-consumer.yml [--runner ubuntu-24.04-arm]
set -euo pipefail

binary=""
output=""
runner="ubuntu-24.04"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --binary) binary="${2:?--binary needs a value}"; shift 2 ;;
    --output) output="${2:?--output needs a value}"; shift 2 ;;
    --runner) runner="${2:?--runner needs a value}"; shift 2 ;;
    -h|--help) sed -n '2,13p' "$0"; exit 0 ;;
    *) echo "check-github-workflow-template: unknown argument $1" >&2; exit 2 ;;
  esac
done

if [ -z "$binary" ] || [ -z "$output" ]; then
  echo "check-github-workflow-template: --binary and --output are required" >&2
  exit 2
fi
if [ ! -x "$binary" ]; then
  echo "check-github-workflow-template: $binary is not executable" >&2
  exit 2
fi
binary="$(cd "$(dirname "$binary")" && pwd)/$(basename "$binary")"

repository_root="$(cd "$(dirname "$0")/.." && pwd)"
fixture="$(mktemp -d "${TMPDIR:-/tmp}/memoria-workflow-fixture-XXXXXX")"
home="${fixture}/home"
project="${fixture}/project"
mkdir -p "$home" "$project"
trap 'rm -rf "$fixture"' EXIT

case "$project" in
  "$repository_root"|"$repository_root"/*)
    echo "check-github-workflow-template: refusing to use a fixture inside the repository" >&2
    exit 2
    ;;
esac

# The fixture uses no host Git configuration, so a developer setting cannot
# change the result.
export GIT_CONFIG_NOSYSTEM=1
export GIT_CONFIG_SYSTEM=/dev/null
export GIT_CONFIG_GLOBAL=/dev/null
export HOME="$home"
export XDG_CONFIG_HOME="${home}/.config"

git -C "$project" init -q
git -C "$project" config user.email fixture@example.com
git -C "$project" config user.name Fixture

cat > "${project}/README.md" <<'MD'
# Consumer fixture

This README owns every selected file in this throwaway project.
MD

"$binary" --root "$project" init --apply > /dev/null

echo "check-github-workflow-template: preview"
"$binary" --root "$project" integrations github install --runner "$runner"

echo "check-github-workflow-template: apply"
"$binary" --root "$project" integrations github install --runner "$runner" --apply > /dev/null

generated="${project}/.github/workflows/memoria.yml"
record="${project}/.github/memoria-workflows/memoria.yml.json"
test -f "$generated"
test -f "$record"

# The project's own check must reach a defined state: the new files make the
# documentation pending until the consumer reviews it.
set +e
"$binary" --root "$project" check --format json > /dev/null
check_status=$?
set -e
echo "check-github-workflow-template: memoria check exits ${check_status} (1 means review is pending)"
if [ "$check_status" != "0" ] && [ "$check_status" != "1" ]; then
  echo "check-github-workflow-template: unexpected check status ${check_status}" >&2
  exit 1
fi

mkdir -p "$(dirname "$output")"
cp "$generated" "$output"
echo "check-github-workflow-template: wrote ${output}"

if command -v python3 > /dev/null 2>&1 \
  && python3 -c 'import yaml' > /dev/null 2>&1; then
  python3 "${repository_root}/tests/setup_action/check_consumer_workflow.py" "$output"
else
  echo "check-github-workflow-template: PyYAML is absent; the independent parse was skipped" >&2
fi
