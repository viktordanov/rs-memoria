# Integrations

**Takeaway:** `memoria integrations` groups the three things Memoria can install for you: an agent skill, an agent hook, and a GitHub Actions workflow. Every operation is explicit, reversible, and local. No operation reviews your documentation.

## Contents

1. [Mental model](#1-mental-model)
2. [Command map](#2-command-map)
3. [Compatible command names](#3-compatible-command-names)
4. [What each integration changes](#4-what-each-integration-changes)
5. [Limits](#5-limits)

## 1. Mental model

Memoria has three integration branches. Each one connects Memoria to a different tool:

- The **skill** teaches an agent the review procedure. The agent reads a Markdown package.
- The **hook** reports the documentation state at the end of an agent turn.
- The **github** branch creates and maintains one continuous-integration workflow file.

All three are optional. Memoria works completely without them.

The GitHub branch has a second part that lives outside your project. The workflow file that Memoria writes calls the first-party setup Action. The Action downloads a verified prebuilt Memoria executable on the runner. The two parts are complementary:

| Component | Owner | What it does |
| --- | --- | --- |
| `memoria integrations github` | Your project | Creates and maintains the workflow file and its ownership record. |
| `viktordanov/rs-memoria` setup Action | The runner | Downloads, verifies, and installs the executable, then puts it on `PATH`. |

Neither component initializes a project, and neither one reviews documentation.

## 2. Command map

```sh
memoria integrations skill  install|status|upgrade|uninstall
memoria integrations hook   install|status|uninstall|run
memoria integrations github install|status|upgrade|uninstall
```

The skill and hook branches keep their current behavior. The [agent integrations guide](agents.md) describes them in full. The [GitHub Actions guide](github-actions.md) describes the workflow branch and the setup Action.

Two concrete examples:

```sh
memoria integrations skill install --target codex
memoria integrations hook install --target claude
```

The hook branch has no `upgrade` operation, and the skill branch has no `run` operation. Memoria adds no operation for symmetry alone. To move an installed hook, uninstall it and install it again.

## 3. Compatible command names

The older `memoria agent ...` names remain supported. They take the same arguments, produce the same data, return the same diagnostics, and exit with the same status.

| Older name | Preferred name |
| --- | --- |
| `memoria agent install` | `memoria integrations skill install` |
| `memoria agent status` | `memoria integrations skill status` |
| `memoria agent upgrade` | `memoria integrations skill upgrade` |
| `memoria agent uninstall` | `memoria integrations skill uninstall` |
| `memoria agent hook install` | `memoria integrations hook install` |

`memoria agent hook status`, `uninstall`, and `run` follow the same pattern.

The JSON envelope of an equivalent command reports the established `agent ...` label under both spellings. A script that matches on `command` keeps working. There is no `memoria integrations agent` level and no `memoria integrations skill hook` path.

An installed hook launcher keeps the `memoria agent hook run` text. An existing installation therefore needs no migration, and a launcher that Memoria 0.5.0 writes still works with an older executable.

## 4. What each integration changes

| Branch | Files it owns | Effect on review state |
| --- | --- | --- |
| skill | The package directory, its record, its backups, and its lock | None. The package is agent context. |
| hook | The client configuration entry and the hook record | None. The record is agent context. |
| github | The workflow file and its ownership record | The two new files are ordinary documentation inputs. |

Install and upgrade in the `github` branch need an initialized project, because the generated job runs `memoria check`. The skill and hook branches need none.

The skill and the hook stay outside the documentation inputs, so an installation never changes freshness. The workflow file and its ownership record belong in your Git history, so they are ordinary inputs. A new workflow makes the owning README pending. Review follows the change, as it does for any other new file.

No integration command acknowledges a review. Only `memoria ack` does that.

## 5. Limits

These items are outside this release:

- Other continuous-integration systems.
- Other agents, other hook events, and global hooks.
- Automatic adoption of a workflow file that Memoria does not own.
- Automatic acknowledgement after a workflow change.

**Next:** invoke `memoria integrations github status` to see the workflow state without changing anything.
