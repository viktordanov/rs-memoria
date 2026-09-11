# GitHub Actions

**Takeaway:** Memoria writes one small workflow file for your repository. That workflow calls the `setup-memoria` Action, which downloads a verified prebuilt executable on an Ubuntu runner. Nothing here compiles Memoria from source, and nothing here reviews your documentation.

**Before you use the remote examples:** version 0.5.0 is not published yet. The `viktordanov/rs-memoria@v0.5.0` reference and the two release archives do not resolve until the maintainer publishes that tag and both architecture pairs. Until then, a generated workflow is correct but its job cannot install Memoria.

## Contents

1. [Mental model](#1-mental-model)
2. [First installation](#2-first-installation)
3. [The generated workflow](#3-the-generated-workflow)
4. [Ownership and your own edits](#4-ownership-and-your-own-edits)
5. [Versions and pins](#5-versions-and-pins)
6. [The setup Action](#6-the-setup-action)
7. [What the checksum proves](#7-what-the-checksum-proves)
8. [States and exit statuses](#8-states-and-exit-statuses)
9. [Limits](#9-limits)

## 1. Mental model

Two components do different work:

- The **CLI** creates and maintains the workflow file in your project. It needs no network and no GitHub credentials.
- The **Action** runs on the GitHub runner. It downloads one release archive, verifies it, and puts the executable on `PATH`.

The CLI never initializes a project, never reviews documentation, and never commits, pushes, or starts a workflow. You commit the generated files through your own Git process.

## 2. First installation

A first-time consumer does these steps in order:

1. Write the root `README.md` of the project.
2. Invoke `memoria init --apply` to create the configuration and the state file.
3. Complete the documented review with `memoria review` and `memoria ack`.
4. Invoke `memoria integrations github install` and read the preview.
5. If the preview is correct, invoke `memoria integrations github install --apply`.

Steps 1 to 3 are prerequisites, not advice. Install and upgrade refuse to write while the root README, `memoria.toml`, or `memoria.lock` is missing or invalid, because the generated job runs `memoria check`. A preview works before initialization and names each missing item. A pending review is permitted: review follows the workflow change. Status and uninstall work in any project, even an uninitialized one.

The fifth command writes two files: the workflow and its ownership record. Examine that difference, review the documentation that the new files affect, then commit the workflow, the record, the configuration, and the reviewed lock.

Install, upgrade, and uninstall show a preview and write nothing without `--apply`. The option `--dry-run` selects the same preview explicitly. `--apply` and `--dry-run` together are an argument error.

An apply recomputes its plan from the bytes on disk at that moment. An earlier preview never permits an overwrite of a later edit.

## 3. The generated workflow

The default destination is `.github/workflows/memoria.yml`. A custom `--path` must name one direct `.yml` or `.yaml` child of `.github/workflows`.

The workflow body comes from one embedded template. The `@v0.5.0` reference in it resolves after publication:

```yaml
name: Memoria documentation
on: [push, pull_request]
permissions:
  contents: read
jobs:
  memoria:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
      - name: Set up Memoria
        id: memoria
        uses: viktordanov/rs-memoria@v0.5.0
        with:
          version: '0.5.0'
      - run: memoria --version
      - run: memoria check
```

The job asks for read permission only. It does not use `pull_request_target`, it needs no secret, and it never acknowledges a review. A pending `memoria check` means that a person must review the documentation locally.

The preview lists the other workflow files that already exist beside the destination. Memoria reads none of them. A new Memoria job can therefore duplicate a documentation check that one of those workflows already runs.

## 4. Ownership and your own edits

Memoria records what it wrote in `.github/memoria-workflows/<filename>.json`. Both files belong in your Git history. The record holds these fields:

- The schema version and the template version.
- The exact workflow path.
- The installer version and the binary version.
- The Action reference and the runner label.
- The expected workflow bytes.

Memoria owns a workflow only when a valid record names it. Memoria never adopts a file without that record, even when the bytes match the current template exactly.

| State | What Memoria found | What a mutation does |
| --- | --- | --- |
| `absent` | No workflow and no record | Install creates both. Uninstall succeeds with no change. |
| `current` | A managed file that matches the target | Install and upgrade succeed with no change. |
| `outdated` | A managed file with other recorded values | Upgrade rewrites it. Install asks you to upgrade. |
| `modified` | A managed file that changed after Memoria wrote it | Every mutation preserves it and fails. |
| `unmanaged` | A workflow without a valid record | Every mutation preserves it and fails. |
| `conflict` | An inconsistent or unreadable pair | Every mutation preserves both files and fails. |

To keep your own edits, manage that workflow yourself, or choose another `--path` for the managed one. Memoria has no `--force` option, writes no automatic backup over your file, and merges no YAML.

A mutation moves the file it replaces into private Git metadata before the replacement appears. If the bytes changed between the plan and the move, Memoria puts them back and reports a conflict. The displaced copies stay as recovery files, and this release removes none of them automatically.

The advisory lock serializes Memoria writers only. It does not stop an arbitrary editor. Memoria therefore compares the bytes again immediately before every write.

## 5. Versions and pins

Two pins are separate. The `version` input selects the Memoria executable. The Action reference selects the Action code.

```sh
memoria integrations github install --version 0.5.0 --action-ref v0.5.0 --runner ubuntu-24.04
```

| Option | Default | Accepted values |
| --- | --- | --- |
| `--version` | The running Memoria version | An exact stable version, 0.5.0 or later, with an optional leading `v` |
| `--action-ref` | `v<version>` | A full 40-character commit SHA, or an exact `vX.Y.Z` tag |
| `--runner` | `ubuntu-24.04` | `ubuntu-24.04`, `ubuntu-latest`, `ubuntu-24.04-arm` |

An upgrade keeps the recorded runner unless you give `--runner`. If the recorded Action reference is a release tag, an upgrade advances it to the new version tag. If the recorded reference is a commit SHA, an upgrade keeps that SHA and says so in the preview. Memoria refuses a downgrade of the binary version.

GitHub recommends a full commit SHA for an Action reference. The generated workflow uses a version tag by default, because a release cannot contain its own final commit SHA. To pin the Action by commit, give a reviewed SHA with `--action-ref`.

If you give `--runner ubuntu-24.04-arm`, the job runs on ARM64 and the Action selects the ARM64 archive automatically. You do not select an architecture yourself.

Memoria does not contact GitHub during a preview or an apply. It cannot confirm that the release assets exist, and the preview says so. For version 0.5.0 the answer is already known: the release is not published yet.

## 6. The setup Action

```yaml
# This reference resolves after the maintainer publishes v0.5.0.
- uses: viktordanov/rs-memoria@v0.5.0
  with:
    version: '0.5.0'
    sha256: ''   # optional, for the runner's own architecture
```

The Action is a composite Action. One Bash step runs `scripts/setup-memoria.py` with Python 3 and `curl`. It installs no Rust toolchain, no Python package, and no agent integration.

| Runner label | Architecture | Release target |
| --- | --- | --- |
| `ubuntu-24.04` | x64 | `x86_64-unknown-linux-gnu` |
| `ubuntu-latest` | x64 | `x86_64-unknown-linux-gnu` |
| `ubuntu-24.04-arm` | ARM64 | `aarch64-unknown-linux-gnu` |

The Action selects the target from the actual process architecture and compares that with `RUNNER_ARCH`. A disagreement is a refusal. The Action supports Ubuntu 24.04 only, and it refuses another release rather than warning about it. Nobody has validated the Action on a newer image, and the `ubuntu-latest` alias can move. Ubuntu 22.04, container jobs, Windows, and macOS are outside this release.

The installer does this work in order:

1. It validates the version, the optional digest, the operating system, and the architecture.
2. It downloads the checksum sidecar, then the archive, from the fixed publisher URL.
3. It compares the archive bytes with the sidecar digest, and with your `sha256` input when you give one.
4. It inspects every archive member, then copies only the executable into a new private directory at mode 0755.
5. It examines the ELF machine identity and requires exact equality from `memoria --version`.
6. Only then it appends the directory to `GITHUB_PATH` and writes the `version` and `path` outputs.

Every validation failure stops before step 6, so no `PATH` entry and no output exists for an archive, an executable, or a version that did not pass. Step 6 itself writes two runner files in order. If the second write fails, `PATH` can already hold the entry while the step still fails with `runner_incomplete`. The Action reports that failure; it does not roll the first write back. There is no Cargo fallback and no Cargo input.

The step outputs are `version` and `path`. The `path` output is the absolute executable path. The current step must use that path. Later steps get the executable on `PATH`.

Bounds: 32 MiB for the download, 64 MiB for the expansion, 16 archive members, 1 KiB for the sidecar, and 30 seconds for archive processing. Transfers use a 10-second connection limit and a 60-second limit for each attempt. A transient network error, a 429 response, and a 5xx response get at most three attempts. A 404 response and every validation error get one.

## 7. What the checksum proves

HTTPS and a same-release checksum find corruption in transfer. They do not authenticate a publisher, and they do not prevent replacement of the archive and the sidecar together. A `sha256` input that you reviewed gives an independent expected value. The Action claims no signature and no attestation.

The `sha256` input applies to the archive for the runner's own architecture. No digest from x64 can validate ARM64 bytes. A matrix over architectures must give one digest for each architecture.

## 8. States and exit statuses

| Exit | Meaning for `memoria integrations github` |
| --- | --- |
| 0 | The preview, the status, or the change succeeded. |
| 1 | No managed workflow exists, or an install found values that need an upgrade. |
| 2 | The arguments are invalid, or the request is a downgrade. |
| 3 | Ownership, modification, a lock, or an interrupted transaction prevents the change. |
| 4 | A filesystem error prevents the operation. |

`status` is always read-only. A readable status exits 0 for every state, a conflict included. The JSON envelope uses the command labels `integrations github install`, `status`, `upgrade`, and `uninstall`, and it holds the `applied`, `dry_run`, and `plan` fields.

## 9. Limits

These items are outside this release:

- A persistent executable cache on the runner.
- A custom repository, a mirror, or an arbitrary download URL.
- Automatic adoption, merge, or repair of a workflow that Memoria does not own.
- Automatic removal of the displaced recovery files.
- Ubuntu 22.04, any release above 24.04, container jobs, self-hosted runners, Windows, and macOS.
- A rollback of the `PATH` entry when the output file rejects a write.

**Next:** invoke `memoria integrations github install` to read the preview for your project.
