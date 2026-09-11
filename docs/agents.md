# Agent integrations

**Takeaway:** Memoria can install a skill package and one `Stop` hook for Codex or Claude. Both integrations are explicit, reversible, and local. Neither one creates a review or continues an agent turn.

## Contents

1. [Mental model](#1-mental-model)
2. [Skill destinations](#2-skill-destinations)
3. [Skill lifecycle](#3-skill-lifecycle)
4. [Retained files](#4-retained-files)
5. [Hook destinations](#5-hook-destinations)
6. [Hook lifecycle](#6-hook-lifecycle)
7. [What the hook reports](#7-what-the-hook-reports)
8. [Limits](#8-limits)

## 1. Mental model

The two integrations do different work:

- The **skill** teaches an agent the review procedure. It is a Markdown package that the agent reads.
- The **hook** reports the current documentation state at the end of an agent turn. It is one command entry in the client's configuration.

Both are optional. Memoria works completely without them.

The preferred command names are `memoria integrations skill ...` and `memoria integrations hook ...`. The `memoria agent ...` names in this guide remain supported, with the same arguments, data, diagnostics, and exit statuses. The [integrations guide](integrations.md) maps the two spellings and describes the third branch, `memoria integrations github`.

## 2. Skill destinations

The default scope is `local`, which means inside the selected Git worktree.

| Target | Local package | Global package |
| --- | --- | --- |
| Codex | `<worktree>/.agents/skills/memoria/` | `$HOME/.agents/skills/memoria/` |
| Claude | `<worktree>/.claude/skills/memoria/` | `$CLAUDE_CONFIG_DIR/skills/memoria/`, or `$HOME/.claude/skills/memoria/` |

The Codex user destination does not follow `CODEX_HOME`.

A global operation requires `--scope global` and an explicit `--target`. It works outside a Git repository and without `memoria.toml`. It rejects `--root`, which has no meaning for that scope.

`--path` names the skills parent directory, not the `memoria` package itself. It always requires `--target`. A local custom path must stay inside the selected worktree. A global custom path must be absolute.

CAUTION: A path outside the worktree never implies a global installation. Pass `--scope global` when you mean it.

## 3. Skill lifecycle

There are four operations:

```text
memoria agent install   [--target codex|claude] [--scope local|global] [--path DIR] [--replace-existing] [--dry-run]
memoria agent status    [--target codex|claude] [--scope local|global] [--path DIR]
memoria agent upgrade   [--target codex|claude] [--scope local|global] [--path DIR] [--dry-run]
memoria agent uninstall [--target codex|claude] [--scope local|global] [--path DIR] [--dry-run]
```

Each operation reports one of six states: `absent`, `current`, `outdated`, `modified`, `unmanaged`, or `conflict`.

This table gives the result of each operation against each state:

| Operation | Absent | Unchanged managed | Older managed | Modified or foreign |
| --- | --- | --- | --- | --- |
| `status` | Reports `absent`. | Reports `current`. | Reports `outdated`. | Reports the state and the paths. |
| `install` | Installs the package. | Exits 0 without writes. | `skill_upgrade_required`, exit 1. | Preserves files. Needs `--replace-existing` for an unmanaged package. |
| `upgrade` | `skill_not_installed`, exit 1. | Exits 0 without writes. | Replaces verified managed content. | `skill_conflict`, exit 3. |
| `uninstall` | Exits 0 with `no_change`. | Removes owned content, restores a user backup. | Applies the same removal. | `skill_conflict`, exit 3. |

`--replace-existing` applies only to an unmanaged directory during install. It creates a verified sibling backup first. It never overwrites another target's managed package, edited managed content, a symlink, or a special file.

`--dry-run` lists every proposed write, removal, replacement, backup, and retained file. It changes nothing.

Status exits 0 for any readable inspection, including an absent or modified package. A read failure exits 4. Status never repairs an installation.

### Backups

When install replaces an unmanaged directory, that original content moves to a sibling backup. Uninstall restores it once.

An upgrade keeps only that original backup. It removes the previous Memoria version after the new package reaches its durable state, so backups do not accumulate. Before any removal, Memoria follows the recorded chain and rejects cycles, escaped paths, changed hashes, and foreign targets.

Uninstall never restores an obsolete Memoria package. Unrelated backups stay untouched and appear as retained files.

## 4. Retained files

Uninstall leaves the zero-byte parent lock in place, named `memoria.install.lock`. This is deliberate. If Memoria deleted the lock pathname, a second process could lock a different inode during a concurrent operation.

The lifecycle report names the lock with reason `synchronization_lock` and `removable_by_uninstall: false`.

An absent uninstall creates no directory and no lock. An existing lock alone still produces a successful, idempotent uninstall. Automatic lock cleanup is outside this release.

The report also explains scope overlap. A global Claude skill can hide a local skill with the same name, and Codex can expose several same-name entries across its discovery paths. Memoria reports the copies it finds and preserves the other scope.

## 5. Hook destinations

The first hook release supports project installation only:

```text
memoria agent hook install   --target codex|claude [--dry-run]
memoria agent hook status    --target codex|claude
memoria agent hook uninstall --target codex|claude [--dry-run]
memoria agent hook run       --target codex|claude --protocol 1 --configuration-root DIR
```

| Target | Configuration destination |
| --- | --- |
| Codex | `<worktree>/.codex/hooks.json`, unless `.codex/config.toml` already holds inline hooks. |
| Claude | `<main-checkout>/.claude/settings.local.json`. |

For Codex, an existing nonempty inline hook table selects the TOML representation. If both project representations hold hooks, install returns `hook_configuration_ambiguous` with exit 3. Memoria does not migrate hooks between files and does not create a second active representation.

For Claude, current documentation places local settings at the main checkout. From a linked worktree, status can inspect that location, but a mutation returns `hook_configuration_outside_worktree` with exit 3. The diagnostic names the destination and gives the `--root` command that works.

`memoria agent hook run` is the documented native endpoint. It reads native hook JSON on stdin and writes one JSON object on stdout. It does not accept `--format`.

## 6. Hook lifecycle

Install requires valid version 2 project configuration. Status and uninstall stay available without valid configuration or state.

Install probes the client version first, without starting an agent session. A missing or older client returns `hook_client_unsupported` with exit 1, before any write. The compatibility floor is Codex 0.153.0 and Claude Code 2.1.259.

Memoria adds one `Stop` group with one command handler, no matcher, and a five-second timeout. The command runs the absolute path of the installing executable:

```sh
memoria_hook_output=$('/absolute/path/to/memoria' agent hook run --target codex --protocol 1 --configuration-root '/absolute/project/root' 2>/dev/null) && printf '%s\n' "$memoria_hook_output" || printf '{}\n'
```

The wrapper captures the output before it writes stdout. If the executable fails, the wrapper emits `{}` and exits 0. An older executable's usage exit never becomes the client's continuation signal.

CAUTION: These are local installation changes. An absolute executable path does not belong in a portable shared hook configuration. Reinstall the hook after the executable moves.

An ownership record sits beside the client configuration, at `.codex/memoria-hook.json` or `.claude/memoria-hook.json`. It records the target, the event, the protocol, the relative configuration path, the exact owned group, a hash of that group, and which containers Memoria created.

Memoria never adopts a user entry. An identical-looking entry without a record returns `hook_unmanaged` with exit 3. A modified owned entry, a duplicate owned entry, or a corrupt record returns `hook_conflict` with exit 3.

Ownership follows the recorded group, not the command that the current executable would install. After the executable moves, status reports the state `relocated`. Uninstall and reinstall both still work on that recorded group.

Install examines the container shapes before it changes anything. An unsupported root value, `hooks` value, or `hooks.Stop` value returns `hook_configuration_invalid` with exit 3, before any write. Memoria never replaces a value that it does not recognize.

Each install and uninstall holds one private lock for its own target, under the worktree's Git metadata directory. A linked worktree therefore gets its own lock and its own transaction record.

Memoria writes a durable intent before each configuration change. That record holds two identities: the bytes the operation requires before its write, and the bytes and ownership it intends to leave behind. Memoria replaces a configuration only when the current bytes still match the required digest, so a concurrent edit returns `hook_conflict` with exit 3 and changes nothing.

If a process stops during an installation, the next explicit install or uninstall settles that transaction first, under the lock. Recovery compares the file it finds with both identities:

- The intended bytes: the write landed, so the remaining ownership step is finished with its recorded provenance.
- The required bytes: the write never landed, so the file stays exactly as found.
- Neither: someone edited the file in that window, so nothing changes. The command returns `hook_conflict` with exit 3 and names the recovery record.

The ownership record carries the same two identities, and absence is one of them. Recovery accepts only the states its phase permits. An ownership record changed during the interruption returns `hook_conflict` with exit 3, and every file keeps its bytes. Ordinary install and uninstall compare the record the same way before they replace or remove it.

An outstanding transaction is never reported as a successful no-op. `memoria agent hook status` and `--dry-run` report the pending record and change nothing.

Uninstall removes only the unchanged owned group. It preserves every other handler, event, and key, and it removes a container only when Memoria created it and it is now empty.

Memoria does not set trust flags, change permissions, enable bypass options, or disable another hook. The client still reviews and activates the hook. Install reports `activation: "requires-client-review"` and points at the client's `/hooks` interface.

## 7. What the hook reports

The runner reads the event name, the working directory, and the `stop_hook_active` flag. It does not open the transcript and does not inspect the last assistant message.

It accepts at most 64 KiB of event JSON. It resolves the event working directory to a Git worktree root and compares that root with the registered configuration root through Git metadata. An event outside the registered project returns `{}` before any project scan.

| State | Result |
| --- | --- |
| Current project without advisories | `{}`, exit 0. |
| Pending, never-reviewed, waiting, or open invalidation work | One `systemMessage` with counts and `memoria review`. |
| Guidance changed without due reviews | One `systemMessage` with `memoria guidance`. |
| Invalid configuration or corrupt state | One `systemMessage` that names the diagnostic code. |
| Deadline or output limit | One `systemMessage` that gives `memoria status` as the manual action. |

The message is at most 512 UTF-8 bytes. It holds no source, guidance, transcript, or secret configuration text.

The runner invokes its own executable as `memoria --root <root> status --summary --format json`, with a two-second deadline and a 16 KiB output limit. The outer runner has a three-second deadline. The five-second client timeout is the final limit.

The runner always exits 0. It never emits a blocking decision, an extra prompt, or a tool request. It never runs a render, an acknowledgement, an invalidation, an installation, or a Git write, and it never reports a failed inspection as a clean project.

## 8. Limits

These integration paths are reserved context in every snapshot, whether installed or absent:

| Category | Paths |
| --- | --- |
| Codex hooks | `.codex/hooks.json`, `.codex/config.toml`, `.codex/memoria-hook.json` |
| Claude hooks | `.claude/settings.local.json`, `.claude/memoria-hook.json` |
| Skill packages | The package, its transaction files, its backups, and its lock |

Memoria labels these files as agent context, not product inputs, so installing an integration never changes documentation freshness. It does not ignore all of `.codex`, `.claude`, or `.agents`. Unrelated files in those directories stay ordinary inputs.

Installation does not modify `.gitignore`, host ignore files, or `.git/info/exclude`, and Memoria never stages these files.

These items are outside this release: global hooks, other hook events, other agents, isolated Claude hooks for individual linked worktrees, automatic client trust, plugin installation, and automatic cleanup of installation locks.

**Next:** invoke `memoria integrations skill status --target codex` to see the current state without changing anything.
