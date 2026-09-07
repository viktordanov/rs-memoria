# Command entry point

The `memoria` executable connects command-line arguments to application calls and results for the user.

This directory owns argument parsing, adapter assembly, and output delivery.
An adapter supplies an external operation, such as file access, through an application contract.
The composition root constructs these adapters and connects them to application functions.
The application and domain determine review state.

Read [main.rs](main.rs#L82) to start with the command dispatch.

## Role in the project

<!-- memoria:export id="summary" -->
The executable parses arguments, constructs adapters, and calls the application.
It writes human text or JSON and returns a process exit status.
<!-- /memoria:export -->

## Arguments become an outcome

`Cli` parses the command, document path, and flags.
`discover_root` identifies the Git worktree root.
`run` creates `Services` from the concrete adapters and dispatches the command.
The application function returns structured data and diagnostics.
The presentation code describes those values without recalculating review decisions.

A review packet contains one README and its input bytes for one review.
An acknowledgement is the saved result of that review.
For a focused review, the executable obtains an encoded packet before it selects the output format.
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
| [main.rs](main.rs#L82) | Adapter assembly, command dispatch, and output delivery |
| [presentation/cli.rs](presentation/cli.rs#L13) | Command grammar |
| [presentation/text.rs](presentation/text.rs#L319) | Human output |
| [presentation/json.rs](presentation/json.rs#L7) | JSON envelope |

An agent skill is a file of instructions for an agent.
`main.rs` embeds `skills/memoria/SKILL.md` at build time.
The installer receives that embedded text through `FsSkillStore`.
A later edit to the source skill requires a new binary build to change the installed text.

## Continue

Read [main.rs:82](main.rs#L82) to trace the adapter assembly.
