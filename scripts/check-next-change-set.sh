#!/bin/sh
# Acceptance checks for the 0.2.0 change set.
#
# The script builds its own fixtures under a unique temporary directory and
# removes only that directory. It never stages, commits, or pushes anything in
# the developer's checkout, and it configures no remote.
set -eu

binary=""
agent_tests="simulated"

usage() {
    cat <<'USAGE'
Usage: check-next-change-set.sh --binary <path> [--agent-tests simulated|none]

  --binary       The release Memoria executable. A relative path resolves
                 against the current directory before the script continues.
  --agent-tests  simulated (default) runs native hook fixtures without a
                 model provider. none skips the integration section.
USAGE
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --binary) binary="${2:?--binary needs a path}"; shift 2 ;;
        --agent-tests) agent_tests="${2:?--agent-tests needs a value}"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) printf 'unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
done

if [ -z "$binary" ]; then
    printf 'error: --binary is required\n' >&2
    usage >&2
    exit 2
fi

case "$agent_tests" in
    simulated|none) ;;
    *) printf 'error: --agent-tests must be simulated or none\n' >&2; exit 2 ;;
esac

# Resolve the binary before any directory change.
case "$binary" in
    /*) MEMORIA="$binary" ;;
    *) MEMORIA="$(pwd)/$binary" ;;
esac

if [ ! -x "$MEMORIA" ]; then
    printf 'error: %s is not an executable file\n' "$MEMORIA" >&2
    exit 2
fi

REPO="$(pwd)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/memoria-acceptance.XXXXXXXX")"
HOME_DIR="$WORK/home"
mkdir -p "$HOME_DIR"

cleanup() {
    rm -rf "$WORK"
}
trap cleanup EXIT INT TERM

failures=""
checks=0

fail() {
    failures="$failures $1"
    printf 'FAIL  %s: %s\n' "$1" "${2:-}" >&2
}

pass() {
    checks=$((checks + 1))
    printf 'ok    %s\n' "$1"
}

# Run Memoria with an isolated home so no developer setting reaches a fixture.
mem() {
    env -u CLAUDE_CONFIG_DIR -u CODEX_HOME \
        HOME="$HOME_DIR" \
        XDG_CONFIG_HOME="$HOME_DIR/.config" \
        GIT_CONFIG_NOSYSTEM=1 \
        "$MEMORIA" "$@"
}

expect_exit() {
    name="$1"; want="$2"; shift 2
    set +e
    output="$("$@" 2>&1)"
    got=$?
    set -e
    if [ "$got" -eq "$want" ]; then
        pass "$name"
    else
        fail "$name" "expected exit $want, got $got: $output"
    fi
}

expect_contains() {
    name="$1"; needle="$2"; shift 2
    set +e
    output="$("$@" 2>&1)"
    set -e
    case "$output" in
        *"$needle"*) pass "$name" ;;
        *) fail "$name" "output does not contain '$needle'" ;;
    esac
}

printf '== environment ==\n'
printf 'os            %s\n' "$(uname -s -r -m)"
printf 'git           %s\n' "$(git --version)"
printf 'memoria       %s\n' "$(mem --version)"
if command -v cargo >/dev/null 2>&1; then
    printf 'rust          %s\n' "$(cargo --version)"
else
    printf 'rust          not on PATH\n'
fi
printf 'workdir       %s\n' "$WORK"

# ---------------------------------------------------------------- fixtures

new_project() {
    root="$WORK/$1"
    mkdir -p "$root/src"
    cd "$root"
    git init -q .
    git config user.email acceptance@example.com
    git config user.name Acceptance
    git config commit.gpgsign false
    cat > README.md <<'MD'
# Acceptance fixture

This README owns every selected file that no nearer README explains.

<!-- memoria:import src="src/README.md#summary" -->
<!-- /memoria:import -->
MD
    cat > src/README.md <<'MD'
# Source

<!-- memoria:export id="summary" -->
The source directory holds the acceptance fixture code.
<!-- /memoria:export -->
MD
    printf 'fn main() {}\n' > src/main.rs
    git add -A
    git commit -qm seed
    cd "$root"
}

# Extract one string field from a JSON envelope. The CLI emits pretty JSON
# with one field per line, so a line-oriented reader is exact enough here.
json_field() {
    sed -n "s/^ *\"$1\": \"\([^\"]*\)\".*/\\1/p" "$2" | head -1
}

# Follow the review plan until it is empty, acknowledging each ready document.
acknowledge_all() {
    set +e
    attempt=0
    while [ "$attempt" -lt 24 ]; do
        attempt=$((attempt + 1))
        mem review --format json > "$WORK/plan.json" 2>/dev/null
        grep -q '"next_action": null' "$WORK/plan.json" && { set -e; return 0; }
        # `next_ready` is a single scalar; the kind comes from the
        # `next_action` object, not from the task list.
        document="$(sed -n 's/^ *"next_ready": "\([^"]*\)".*/\1/p' "$WORK/plan.json" | head -1)"
        kind="$(sed -n '/"next_action": {/,/}/p' "$WORK/plan.json" \
            | sed -n 's/^ *"kind": "\([^"]*\)".*/\1/p' | head -1)"
        if [ -z "$document" ]; then
            set -e
            return 0
        fi
        if [ "$kind" = "render" ]; then
            mem render "$document" >/dev/null 2>&1
            continue
        fi
        mem review "$document" --format json > "$WORK/packet.json" 2>/dev/null
        token="$(json_field token "$WORK/packet.json")"
        if [ -z "$token" ]; then
            set -e
            fail "acknowledge" "no token for $document"
            return 1
        fi
        mem ack "$document" --packet "$WORK/packet.json" --token "$token" \
            --reviewer "Acceptance" --result no-update \
            --note "The acceptance fixture explanation matches its reviewed inputs." \
            >/dev/null 2>&1
    done
    set -e
    fail "acknowledge" "the review plan did not empty in 24 steps"
    return 1
}

# -------------------------------------------------------- setup and state

printf '\n== setup ==\n'
new_project setup

before="$(find . -path ./.git -prune -o -type f -print | sort)"
expect_exit "init-preview-exit" 0 mem init
after="$(find . -path ./.git -prune -o -type f -print | sort)"
if [ "$before" = "$after" ]; then
    pass "init-preview-writes-nothing"
else
    fail "init-preview-writes-nothing" "the preview created or removed files"
fi
expect_contains "init-preview-explains-choice" "architecture modules" mem init

expect_exit "init-apply-exit" 0 mem init --apply
[ -f memoria.toml ] && pass "init-creates-configuration" || fail "init-creates-configuration" "memoria.toml is missing"
[ -f memoria.lock ] && pass "init-creates-lock" || fail "init-creates-lock" "memoria.lock is missing"
[ -d .memoria ] && fail "init-creates-no-legacy-directory" ".memoria exists" || pass "init-creates-no-legacy-directory"

# The exact filename and its binary framing.
magic="$(head -c 4 memoria.lock | od -An -c | tr -d ' \n')"
case "$magic" in
    'MML\0') pass "lock-magic-bytes" ;;
    *) fail "lock-magic-bytes" "unexpected magic: $magic" ;;
esac

printf '/memoria.lock binary\n' > .gitattributes
git add -A >/dev/null
expect_contains "lock-is-binary-to-git" "-	-	memoria.lock" git diff --numstat --cached

expect_exit "init-apply-idempotent" 0 mem init --apply

# A missing root README refuses before any write.
new_project no-readme
rm README.md
expect_exit "apply-without-root-readme" 1 mem init --apply
[ -f memoria.toml ] && fail "apply-writes-nothing-without-readme" "memoria.toml was created" || pass "apply-writes-nothing-without-readme"

# ------------------------------------------------------------ the workflow

printf '\n== review workflow ==\n'
new_project workflow
mem init --apply >/dev/null
acknowledge_all
expect_exit "workflow-check-clean" 0 mem check
expect_exit "workflow-lint-clean" 0 mem lint
expect_exit "workflow-status-clean" 0 mem status

state_before="$(cksum memoria.lock)"
printf '// changed\n' >> src/main.rs
expect_exit "changed-source-fails-check" 1 mem check
acknowledge_all
expect_exit "review-restores-clean-check" 0 mem check

# ------------------------------------------------------------- portability

printf '\n== portability ==\n'
new_project portability
mem init --apply >/dev/null
acknowledge_all
expect_exit "portable-baseline-clean" 0 mem check
clean_state="$(cksum memoria.lock)"

# Every host ignore source, varied in turn, matching no project input.
printf 'never-matched-a/**\n' > "$WORK/host-excludes"
git config core.excludesFile "$WORK/host-excludes"
expect_exit "host-global-rule-keeps-check-clean" 0 mem check
printf 'never-matched-b/**\n' > "$WORK/host-excludes"
expect_exit "host-global-edit-keeps-check-clean" 0 mem check
mkdir -p .git/info
printf 'never-matched-c/**\n' > .git/info/exclude
expect_exit "host-info-exclude-keeps-check-clean" 0 mem check
mkdir -p "$HOME_DIR/.config/git"
printf 'never-matched-d/**\n' > "$HOME_DIR/.config/git/ignore"
git config --unset core.excludesFile
expect_exit "host-xdg-rule-keeps-check-clean" 0 mem check

if [ "$clean_state" = "$(cksum memoria.lock)" ]; then
    pass "host-rules-rewrite-no-state"
else
    fail "host-rules-rewrite-no-state" "memoria.lock changed"
fi

# A repository rule is real policy.
printf 'generated/**\n' >> .gitignore
expect_exit "repository-rule-changes-policy" 1 mem check
git checkout -- .gitignore 2>/dev/null || printf '' > .gitignore
rm -f .gitignore
expect_exit "repository-rule-restored" 0 mem check

# ---------------------------------------------------------------- guidance

printf '\n== guidance ==\n'
new_project guidance
mem init --apply >/dev/null
cat > memoria.toml <<'TOML'
version = 2
ignore = []
include = []

[documentation]
guidance = ["Explain the operational workflow before implementation details."]
guidance_files = []
TOML
acknowledge_all
expect_exit "guidance-baseline-clean" 0 mem check
expect_exit "guidance-command-exit" 0 mem guidance
expect_contains "guidance-command-shows-text" "operational workflow" mem guidance README.md

guidance_state="$(cksum memoria.lock)"
cat > memoria.toml <<'TOML'
version = 2
ignore = []
include = []

[documentation]
guidance = ["Explain the operational workflow before the implementation details."]
guidance_files = []
TOML
expect_exit "guidance-change-keeps-check-clean" 0 mem check
expect_contains "guidance-change-is-reported" '"changed_documents": 2' mem check --format json
if [ "$guidance_state" = "$(cksum memoria.lock)" ]; then
    pass "guidance-change-rewrites-no-state"
else
    fail "guidance-change-rewrites-no-state" "memoria.lock changed"
fi

# The retired keys name their replacement.
cat > memoria.toml <<'TOML'
version = 2
[documentation]
instructions = ["Old key."]
TOML
expect_exit "retired-guidance-key-fails" 1 mem status
expect_contains "retired-key-names-replacement" "documentation.guidance" mem status --format json
cat > memoria.toml <<'TOML'
version = 1
TOML
expect_exit "version-one-fails" 1 mem status
expect_contains "version-one-names-cutover" "docs/releases/0.2.0.md" mem status --format json

# ------------------------------------------------------------ clean cutover

printf '\n== clean cutover ==\n'
new_project cutover
mem init --apply >/dev/null
acknowledge_all
mkdir -p .memoria
printf '{"schema_version":1}' > .memoria/state.json
expect_exit "dual-state-fails" 4 mem status
expect_contains "dual-state-diagnostic" "state_ambiguous" mem status --format json
rm memoria.lock
expect_exit "legacy-state-fails" 4 mem status
expect_contains "legacy-state-diagnostic" "state_legacy" mem status --format json
[ -f memoria.lock ] && fail "legacy-state-writes-nothing" "memoria.lock was created" || pass "legacy-state-writes-nothing"
rm -rf .memoria

# ---------------------------------------------------------- state inspection

printf '\n== state inspection ==\n'
new_project inspect
mem init --apply >/dev/null
acknowledge_all
expect_exit "state-inspect-exit" 0 mem state inspect
expect_contains "state-inspect-format" '"format_version": 2' mem state inspect --format json
expect_contains "state-inspect-no-freshness-claim" "memoria status" mem state inspect
inspect_state="$(cksum memoria.lock)"
mem state inspect >/dev/null
if [ "$inspect_state" = "$(cksum memoria.lock)" ]; then
    pass "state-inspect-is-read-only"
else
    fail "state-inspect-is-read-only" "memoria.lock changed"
fi

# The four committed vectors decode through the release CLI, outside Git.
mkdir -p "$WORK/outside"
cd "$WORK/outside"
for vector in empty tiny current mixed; do
    file="$REPO/tests/fixtures/state-v2/$vector.lock"
    if [ ! -f "$file" ]; then
        fail "vector-$vector-present" "missing $file"
        continue
    fi
    expect_exit "vector-$vector-inspects" 0 mem state inspect --file "$file" --format json
done
expect_exit "state-inspect-missing-file" 4 mem state inspect --file "$WORK/outside/absent.lock"
expect_contains "state-inspect-missing-diagnostic" "state_missing" \
    mem state inspect --file "$WORK/outside/absent.lock" --format json

# Corruption is reported and never repaired.
cp "$REPO/tests/fixtures/state-v2/current.lock" "$WORK/outside/broken.lock"
printf 'x' | dd of="$WORK/outside/broken.lock" bs=1 seek=8 conv=notrunc status=none
expect_exit "state-inspect-corruption" 4 mem state inspect --file "$WORK/outside/broken.lock"
expect_contains "state-inspect-corruption-diagnostic" "state_corrupt" \
    mem state inspect --file "$WORK/outside/broken.lock" --format json

# -------------------------------------------------------------- integrations

if [ "$agent_tests" = "simulated" ]; then
    printf '\n== integrations ==\n'
    new_project integrations
    mem init --apply >/dev/null
    acknowledge_all
    integration_state="$(cksum memoria.lock)"

    expect_exit "skill-install" 0 mem agent install --target codex
    [ -f .agents/skills/memoria/SKILL.md ] && pass "skill-package-present" \
        || fail "skill-package-present" "SKILL.md is missing"
    expect_contains "skill-status-current" '"state": "current"' \
        mem agent status --target codex --format json
    expect_exit "skill-uninstall" 0 mem agent uninstall --target codex
    expect_exit "skill-uninstall-idempotent" 0 mem agent uninstall --target codex
    [ -f .agents/skills/memoria.install.lock ] && pass "skill-lock-retained" \
        || fail "skill-lock-retained" "the deliberate lock was removed"
    [ -d .agents/skills/memoria ] && fail "skill-package-removed" "the package remains" \
        || pass "skill-package-removed"

    # A stub client so the version probe finds a supported target.
    mkdir -p "$HOME_DIR/bin"
    printf '#!/bin/sh\necho "codex-cli 0.153.0"\n' > "$HOME_DIR/bin/codex"
    chmod 755 "$HOME_DIR/bin/codex"
    PATH="$HOME_DIR/bin:$PATH"
    export PATH

    expect_exit "hook-install" 0 mem agent hook install --target codex
    [ -f .codex/hooks.json ] && pass "hook-configuration-present" \
        || fail "hook-configuration-present" ".codex/hooks.json is missing"
    [ -f .codex/memoria-hook.json ] && pass "hook-record-present" \
        || fail "hook-record-present" "the ownership record is missing"
    expect_contains "hook-requires-client-review" "requires-client-review" \
        mem agent hook status --target codex

    # The native runner speaks its own protocol and always exits 0.
    root="$(pwd)"
    printf '{"hook_event_name":"Stop","cwd":"%s","stop_hook_active":false}' "$root" > "$WORK/event.json"
    set +e
    runner_output="$(mem agent hook run --target codex --protocol 1 \
        --configuration-root "$root" < "$WORK/event.json" 2>/dev/null)"
    runner_exit=$?
    set -e
    if [ "$runner_exit" -eq 0 ]; then pass "hook-runner-exit"; else
        fail "hook-runner-exit" "expected exit 0, got $runner_exit"
    fi
    case "$runner_output" in
        '{}') pass "hook-runner-silent-when-current" ;;
        *) fail "hook-runner-silent-when-current" "unexpected output: $runner_output" ;;
    esac

    printf '// pending\n' >> src/main.rs
    set +e
    runner_output="$(mem agent hook run --target codex --protocol 1 \
        --configuration-root "$root" < "$WORK/event.json" 2>/dev/null)"
    runner_exit=$?
    set -e
    if [ "$runner_exit" -eq 0 ]; then pass "hook-runner-exit-when-pending"; else
        fail "hook-runner-exit-when-pending" "expected exit 0, got $runner_exit"
    fi
    case "$runner_output" in
        *systemMessage*memoria\ review*) pass "hook-runner-advises-review" ;;
        *) fail "hook-runner-advises-review" "unexpected output: $runner_output" ;;
    esac
    case "$runner_output" in
        *'"decision"'*|*'"block"'*) fail "hook-runner-never-blocks" "the runner emitted a decision" ;;
        *) pass "hook-runner-never-blocks" ;;
    esac

    # A recursion guard and a wrong event stay silent.
    printf '{"hook_event_name":"Stop","cwd":"%s","stop_hook_active":true}' "$root" > "$WORK/recursive.json"
    set +e
    recursive_output="$(mem agent hook run --target codex --protocol 1 \
        --configuration-root "$root" < "$WORK/recursive.json" 2>/dev/null)"
    set -e
    case "$recursive_output" in
        '{}') pass "hook-runner-recursion-guard" ;;
        *) fail "hook-runner-recursion-guard" "unexpected output: $recursive_output" ;;
    esac

    git checkout -- src/main.rs
    expect_exit "hook-uninstall" 0 mem agent hook uninstall --target codex
    [ -f .codex/memoria-hook.json ] && fail "hook-record-removed" "the record remains" \
        || pass "hook-record-removed"

    if [ "$integration_state" = "$(cksum memoria.lock)" ]; then
        pass "integrations-change-no-state"
    else
        fail "integrations-change-no-state" "memoria.lock changed"
    fi
    expect_exit "integrations-keep-check-clean" 0 mem check
else
    printf '\n== integrations skipped (--agent-tests none) ==\n'
fi

# ------------------------------------------------------------------- summary

cd "$REPO"
printf '\n== summary ==\n'
printf 'checks passed %s\n' "$checks"
if [ -n "$failures" ]; then
    printf 'failed:%s\n' "$failures"
    exit 1
fi
printf 'all acceptance checks passed\n'
