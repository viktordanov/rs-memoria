# Command entry point

The `memoria` executable connects command-line arguments to application calls and results for the user.

This directory owns argument parsing, adapter assembly, and output delivery.
An adapter supplies an external operation, such as file access, through an application contract.
The composition root constructs these adapters and connects them to application functions.
The application and domain determine review state.

Read [main.rs](main.rs#L82) to start with the command dispatch.

## On this page

- [Role in the project](#role-in-the-project)
- [Command surface](#command-surface)
- [Arguments become an outcome](#arguments-become-an-outcome)
- [Output and exit statuses](#output-can-fail-after-a-mutation)
- [File map and next step](#file-map)

## Role in the project

<!-- memoria:export id="summary" -->
The executable parses arguments, constructs adapters, and calls the application.
It writes human text or JSON and returns a process exit status.
<!-- /memoria:export -->

## Command surface

The root README imports this block.
This arrangement keeps the public command list with the command entry point.

<!-- memoria:export id="cli-help" -->
```text
Keep a project's documented mental model connected to its code.

Usage: memoria [OPTIONS] <COMMAND>

Commands:
  init        Preview the setup, or create the two committed files with --apply
  status      Show coverage, input size, and review state
  guidance    Show the project documentation guidance that applies to a README
  state       Inspect committed state without changing it
  lint        Check structure, configuration, markers, and link hints
  review      Show the ordered review plan, or a focused packet for one README
  render      Refresh declared import blocks only
  ack         Record a review result against the exact packet snapshot
  invalidate  Mark one README, a subtree, or the whole project for semantic review
  check       Run read-only validation for CI
  graph       Show documentation ownership, imports, navigation, and status
  agent       Install or remove the managed Memoria skill and hooks for an agent
  help        Print this message or the help of the given subcommand(s)

Options:
      --root <DIRECTORY>  Project root. Must be the Git worktree root. Defaults to discovery from the current directory
      --format <FORMAT>   Output format [default: human] [possible values: human, json]
  -h, --help              Print help
  -V, --version           Print version
```
<!-- /memoria:export -->

## Arguments become an outcome

`Cli` parses the command, document path, and flags.
`discover_root` identifies the Git worktree root.
`run` creates `Services` from the concrete adapters and dispatches the command.
The write lock comes from the Git port, at the worktree-private path, so the committed state file is never the process lock.
The application function returns structured data and diagnostics.
The presentation code describes those values without recalculating review decisions.

A review packet contains one README and its input bytes for one review.
An acknowledgement is the saved result of that review.
For a focused review, the executable obtains an encoded packet before it selects the output format.
Thus, packet limits apply equally to human text and JSON.
Only the JSON packet supports acknowledgement.
Human packet text supports reading.

Source evidence: [cli.rs](presentation/cli.rs#L1) and [main.rs](main.rs#L1).

## Output can fail after a mutation

The JSON envelope contains `schema_version`, `command`, `ok`, `data`, and `diagnostics`, at schema version 2.
The native hook runner is the one exception: it writes one native JSON object and always exits 0.
The runner starts one three-second deadline at entry and bounds its input, its discovery, and its child.
One supervisor owns every process it starts, so an expired deadline terminates them before the runner returns.
Its grammar has no format option, so an explicit `--format` value is a usage error.
Human diagnostics go to stderr.
The final response goes to stdout.
Help and version output use plain text.

If stdout fails after a mutation, the process exits 4.
The completed mutation remains in place.
`memoria status` shows the resulting review state.
An output error does not mean that the mutation failed.

Source evidence: [json.rs](presentation/json.rs#L7) and [main.rs](main.rs#L1).

## Exit statuses

An exit status is the number that the process returns to its caller.

| Exit | Meaning |
| --- | --- |
| 0 | The command succeeded. |
| 1 | Project validation failed or required reviews remain. |
| 2 | The command arguments or packet are invalid. |
| 3 | A lock, snapshot, revision, or installation conflict prevents the operation. |
| 4 | An I/O error, unsupported Git state, or corrupt review state prevents success. |

Application exit classes come from [error.rs:185](../crates/memoria-application/src/error.rs#L185).
The executable also owns usage errors and errors during output delivery.

## File map

| File | Ownership |
| --- | --- |
| [main.rs](main.rs#L1) | Adapter assembly, command dispatch, and output delivery |
| [presentation/cli.rs](presentation/cli.rs#L1) | Command grammar |
| [presentation/text.rs](presentation/text.rs#L1) | Human output |
| [presentation/json.rs](presentation/json.rs#L1) | JSON envelope |

An agent skill is a file of instructions for an agent.
`main.rs` embeds `skills/memoria/SKILL.md` at build time.
The installer receives that embedded text through `FsSkillStore`.
A later edit to the source skill requires a new binary build to change the installed text.

Three commands resolve before project discovery, because they do not need a project:

1. `memoria agent hook run` speaks native hook JSON on stdin and stdout.
2. `memoria state inspect --file` decodes one explicit file outside Git.
3. A global `memoria agent` operation builds narrow agent services.

An ordinary command runs no agent client program.
Only a hook installation measures the version of the client that the target selects.

## Continue

Read [main.rs](main.rs#L1) to trace the adapter assembly.
