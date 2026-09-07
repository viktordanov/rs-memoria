# Memoria binary

The binary connects command arguments to application use cases and delivers their outcomes.

This directory is the composition root and presentation boundary.
It creates adapters once for each command invocation.
The application and domain determine review state.

## Role in the project

<!-- memoria:export id="summary" -->
The binary builds the adapters and passes them to an application use case.
It converts the outcome to human text or a JSON envelope.
It also maps application errors and output errors to process exit statuses.
<!-- /memoria:export -->

## Arguments become an outcome

`Cli` parses the command, document path, and flags.
`discover_root` identifies the Git worktree root.
`run` creates `Services` from the concrete adapters and dispatches the command.
The use case returns structured data and diagnostics.
The presentation code describes those values without recalculating review decisions.

For a focused review, the binary obtains an encoded packet before it selects the output format.
Thus, packet limits apply equally to human text and JSON.
Only the JSON packet supports acknowledgement.
Human packet text supports reading.

Source evidence: [cli.rs:13](presentation/cli.rs#L13) and [main.rs:82](main.rs#L82).

## Output can fail after a mutation

The JSON envelope contains `schema_version`, `command`, `ok`, `data`, and `diagnostics`.
Human diagnostics go to stderr.
The final response goes to stdout.
Help and version output use plain text.

If stdout fails after a mutation, the process exits 4.
The completed mutation remains in place.
`memoria status` shows the resulting review state.
An output error does not mean that the mutation failed.

Source evidence: [json.rs:7](presentation/json.rs#L7) and [main.rs:312](main.rs#L312).

## Exit statuses

| Exit | Meaning |
| --- | --- |
| 0 | The command succeeded. |
| 1 | Project validation failed or required reviews remain. |
| 2 | The command arguments or packet are invalid. |
| 3 | A lock, snapshot, revision, or installation conflict prevents the operation. |
| 4 | An I/O error, unsupported Git state, or corrupt review state prevents success. |

Application exit classes come from [error.rs:185](../crates/memoria-application/src/error.rs#L185).
The binary also owns usage errors and errors during output delivery.

## File map

| File | Ownership |
| --- | --- |
| [main.rs](main.rs#L82) | Adapter assembly, command dispatch, and output delivery |
| [presentation/cli.rs](presentation/cli.rs#L13) | Command grammar |
| [presentation/text.rs](presentation/text.rs#L319) | Human output |
| [presentation/json.rs](presentation/json.rs#L7) | JSON envelope |

`main.rs` embeds `skills/memoria/SKILL.md` at build time.
The installer receives that embedded text through `FsSkillStore`.
A later edit to the source skill requires a new binary build to change the installed text.

## Continue

Read [main.rs:82](main.rs#L82) to trace the adapter assembly.
