#!/usr/bin/env python3
"""Install a verified prebuilt Memoria executable on a GitHub Actions runner.

The composite Action calls this program once. The program downloads one
published release archive and its checksum sidecar, verifies both, inspects
every archive member, copies only the executable into a private directory,
and publishes that directory on `PATH`.

Every failure is closed: the program publishes `PATH` and the step outputs
only after the checksum, the archive inspection, the ELF machine check and
the `--version` equality check all pass.

The program reads its environment through an explicit mapping and reads the
process architecture, the operating-system release file and `curl` through
module-level seams. Tests replace those seams; production code has no
test-only environment variable.
"""

from __future__ import annotations

import dataclasses
import gzip
import hashlib
import os
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
import zlib
from typing import Mapping

# ---------------------------------------------------------------------------
# Contract constants
# ---------------------------------------------------------------------------

REPOSITORY = "viktordanov/rs-memoria"
RELEASE_BASE = f"https://github.com/{REPOSITORY}/releases/download"

#: The first version this Action can install.
MINIMUM_VERSION = (0, 5, 0)

#: Compressed download bound.
MAX_ARCHIVE_BYTES = 32 * 1024 * 1024
#: Expanded bytes bound across every inspected member.
MAX_EXPANDED_BYTES = 64 * 1024 * 1024
#: Member-count bound.
MAX_MEMBERS = 16
#: Checksum sidecar bound.
MAX_SIDECAR_BYTES = 1024
#: Whole-archive processing bound, in seconds.
ARCHIVE_DEADLINE_SECONDS = 30

#: Transfer bounds, in seconds.
CONNECT_TIMEOUT_SECONDS = 10
MAX_TIME_SECONDS = 60
#: Attempts for a transient transport failure, a 429 or a 5xx response.
MAX_ATTEMPTS = 3
#: Delay before each retry, in seconds.
RETRY_DELAY_SECONDS = 2

#: The executable must answer `--version` inside this many seconds.
VERSION_DEADLINE_SECONDS = 5
#: Bound on the bytes the version check reads from the child.
MAX_VERSION_OUTPUT_BYTES = 4096

#: The only Ubuntu release this Action claims.
SUPPORTED_UBUNTU = (24, 4)

#: Runtime architecture to release target.
TARGETS = {
    "x86_64": "x86_64-unknown-linux-gnu",
    "aarch64": "aarch64-unknown-linux-gnu",
}

#: Release target to the `RUNNER_ARCH` label GitHub sets.
RUNNER_ARCHES = {
    "x86_64-unknown-linux-gnu": "X64",
    "aarch64-unknown-linux-gnu": "ARM64",
}

#: Release target to the ELF `e_machine` value.
ELF_MACHINES = {
    "x86_64-unknown-linux-gnu": 0x3E,
    "aarch64-unknown-linux-gnu": 0xB7,
}

VERSION_PATTERN = re.compile(r"^v?(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")
DIGEST_PATTERN = re.compile(r"^[0-9a-f]{64}$")

#: Read by [`read_os_release`]. Tests replace this attribute.
OS_RELEASE_PATH = "/etc/os-release"


class SetupError(Exception):
    """A refusal with a stable code. No archive or `PATH` change happened."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code
        self.message = message


# ---------------------------------------------------------------------------
# Seams
# ---------------------------------------------------------------------------


def process_machine() -> str:
    """The actual architecture of this process. Tests replace this."""
    return os.uname().machine


def read_os_release() -> str:
    """The operating-system release description. Tests replace this."""
    with open(OS_RELEASE_PATH, "r", encoding="utf-8", errors="replace") as handle:
        return handle.read()


# ---------------------------------------------------------------------------
# Pure validation
# ---------------------------------------------------------------------------


def normalize_version(raw: str) -> str:
    """Return the exact stable version, without a leading `v`.

    A range, a prerelease, a build identifier, `latest` and any version below
    the minimum are refused.
    """
    text = (raw or "").strip()
    if not text:
        raise SetupError("version_invalid", "version is empty; give an exact version such as 0.5.0")
    match = VERSION_PATTERN.match(text)
    if match is None:
        raise SetupError(
            "version_invalid",
            f"version {text!r} is not an exact stable version; give MAJOR.MINOR.PATCH such as 0.5.0",
        )
    parts = tuple(int(part) for part in match.groups())
    if parts < MINIMUM_VERSION:
        minimum = ".".join(str(part) for part in MINIMUM_VERSION)
        raise SetupError(
            "version_unsupported",
            f"version {text!r} is below {minimum}; this Action installs {minimum} and later",
        )
    return "{}.{}.{}".format(*parts)


def normalize_digest(raw: str | None) -> str | None:
    """Return the optional caller digest in lowercase hexadecimal."""
    if raw is None:
        return None
    text = raw.strip().lower()
    if not text:
        return None
    if DIGEST_PATTERN.match(text) is None:
        raise SetupError(
            "digest_invalid",
            "sha256 must be 64 hexadecimal characters for the selected architecture's archive",
        )
    return text


def parse_os_release(text: str) -> tuple[str, str]:
    """Return `(ID, VERSION_ID)` from an `os-release` description."""
    values: dict[str, str] = {}
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, _, value = line.partition("=")
        values[key.strip()] = value.strip().strip('"').strip("'")
    return values.get("ID", ""), values.get("VERSION_ID", "")


def check_platform(platform: str, os_release: str) -> None:
    """Make sure that the runner is a supported Ubuntu host.

    This Action supports Ubuntu 24.04 only. A different release is a refusal,
    not a warning: no hosted evidence exists for another image, and the
    `ubuntu-latest` alias can move. Validate a new image before you widen this
    gate.
    """
    if not platform.startswith("linux"):
        raise SetupError(
            "platform_unsupported",
            f"this Action supports Linux runners only; this runner reports {platform!r}",
        )
    identifier, version_id = parse_os_release(os_release)
    if identifier != "ubuntu":
        raise SetupError(
            "platform_unsupported",
            f"this Action supports Ubuntu runners only; this runner reports ID={identifier!r}",
        )
    supported = "{}.{:02d}".format(*SUPPORTED_UBUNTU)
    try:
        release = tuple(int(part) for part in version_id.split(".")[:2])
    except ValueError:
        release = ()
    if len(release) < 2 or release != SUPPORTED_UBUNTU:
        raise SetupError(
            "platform_unsupported",
            f"this Action supports Ubuntu {supported} only; this runner reports "
            f"VERSION_ID={version_id!r}. Validate the new image before you widen this gate.",
        )


def select_target(machine: str, runner_arch: str | None) -> str:
    """Return the release target for the actual process architecture.

    The `RUNNER_ARCH` label is a cross-check, never the selection input. A
    disagreement is a refusal, because one of the two facts is wrong.
    """
    target = TARGETS.get(machine)
    if target is None:
        supported = ", ".join(sorted(TARGETS))
        raise SetupError(
            "architecture_unsupported",
            f"architecture {machine!r} has no Memoria Linux release; supported: {supported}",
        )
    declared = (runner_arch or "").strip().upper()
    if declared and declared != RUNNER_ARCHES[target]:
        raise SetupError(
            "architecture_mismatch",
            f"RUNNER_ARCH={declared} disagrees with the process architecture {machine!r} "
            f"(expected {RUNNER_ARCHES[target]}); refusing to guess the release asset",
        )
    return target


def archive_name(version: str, target: str) -> str:
    return f"memoria-{version}-{target}.tar.gz"


def archive_url(version: str, target: str) -> str:
    return f"{RELEASE_BASE}/v{version}/{archive_name(version, target)}"


def parse_sidecar(text: str, expected_name: str) -> str:
    """Return the digest from one `sha256sum` record.

    The record must name the exact archive basename, so a sidecar copied from
    another architecture cannot validate these bytes.
    """
    records = [line for line in text.splitlines() if line.strip()]
    if len(records) != 1:
        raise SetupError(
            "sidecar_invalid",
            f"the checksum sidecar must hold exactly one record; it holds {len(records)}",
        )
    fields = records[0].split()
    if len(fields) != 2:
        raise SetupError(
            "sidecar_invalid",
            "the checksum sidecar record must be a digest followed by the archive name",
        )
    digest, name = fields[0].lower(), fields[1].lstrip("*")
    if DIGEST_PATTERN.match(digest) is None:
        raise SetupError("sidecar_invalid", "the checksum sidecar digest is not 64 hexadecimal characters")
    if name != expected_name:
        raise SetupError(
            "sidecar_mismatch",
            f"the checksum sidecar names {name!r}, not the requested archive {expected_name!r}",
        )
    return digest


def check_command_file(kind: str, path: str | None) -> str:
    """Make sure that a workflow command file is usable before any download."""
    if not path:
        raise SetupError(
            "runner_incomplete",
            f"{kind} is not set; this Action runs inside GitHub Actions only",
        )
    if "\n" in path or "\r" in path:
        raise SetupError("runner_incomplete", f"{kind} contains a line break")
    directory = os.path.dirname(path) or "."
    if not os.path.isdir(directory):
        raise SetupError("runner_incomplete", f"{kind} directory {directory!r} does not exist")
    try:
        with open(path, "a", encoding="utf-8"):
            pass
    except OSError as error:
        raise SetupError("runner_incomplete", f"cannot append to {kind} ({path}): {error}") from None
    return path


def elf_machine(header: bytes) -> int:
    """Return the `e_machine` value of a 64-bit little-endian ELF header."""
    if len(header) < 20 or header[:4] != b"\x7fELF":
        raise SetupError("binary_invalid", "the installed file is not an ELF executable")
    if header[4] != 2:
        raise SetupError("binary_invalid", "the installed file is not a 64-bit ELF executable")
    if header[5] != 1:
        raise SetupError("binary_invalid", "the installed file is not a little-endian ELF executable")
    return int.from_bytes(header[18:20], "little")


# ---------------------------------------------------------------------------
# Archive inspection
# ---------------------------------------------------------------------------


@dataclasses.dataclass(frozen=True)
class ArchiveContract:
    """The member names one release archive must hold."""

    prefix: str
    binary: str
    license: str

    @staticmethod
    def of(version: str, target: str) -> "ArchiveContract":
        prefix = f"memoria-{version}-{target}"
        return ArchiveContract(prefix=prefix, binary=f"{prefix}/memoria", license=f"{prefix}/LICENSE")

    def permitted(self) -> set[str]:
        return {self.prefix, self.binary, self.license}


#: Tar header block size.
TAR_BLOCK = 512

#: The only tar type flags a Memoria release archive may hold: a regular
#: file in either spelling, and a directory.
PERMITTED_TYPEFLAGS = frozenset({b"0", b"\x00", b"5"})


def _bounded_gunzip(archive_path: str, tar_path: str, deadline: float) -> int:
    """Expand one gzip member into `tar_path` under the expansion bound.

    The bound is applied while the stream is read. A gzip bomb therefore stops
    at the bound instead of expanding first and failing afterward.
    """
    written = 0
    with gzip.open(archive_path, "rb") as source, open(tar_path, "wb") as target:
        while True:
            if time.monotonic() > deadline:
                raise SetupError("archive_limit", "archive expansion exceeded its time bound")
            try:
                chunk = source.read(65536)
            except (OSError, EOFError, gzip.BadGzipFile, zlib.error) as error:
                raise SetupError("archive_invalid", f"the archive is not readable gzip: {error}") from None
            if not chunk:
                break
            written += len(chunk)
            if written > MAX_EXPANDED_BYTES:
                raise SetupError(
                    "archive_limit",
                    f"the archive expands beyond {MAX_EXPANDED_BYTES} bytes",
                )
            target.write(chunk)
    if written == 0 or written % TAR_BLOCK != 0:
        raise SetupError("archive_invalid", "the archive is not a whole number of tar blocks")
    return written


def _prescan_tar(tar_path: str, deadline: float) -> None:
    """Examine the raw tar headers before any tar library reads them.

    This pass enforces the member count, the declared expansion and the time
    bound, and it refuses every extended or vendor metadata record. A PAX or
    GNU long-name header can carry an arbitrarily large payload that a tar
    library expands internally, so that payload must never reach the library.
    """
    members = 0
    expanded = 0
    with open(tar_path, "rb") as handle:
        while True:
            if time.monotonic() > deadline:
                raise SetupError("archive_limit", "archive inspection exceeded its time bound")
            header = handle.read(TAR_BLOCK)
            if len(header) < TAR_BLOCK:
                raise SetupError("archive_invalid", "the archive ends inside a tar header")
            if header == bytes(TAR_BLOCK):
                # The end-of-archive marker. Everything after it is padding.
                break
            # Both the POSIX and the GNU spellings start with `ustar`.
            if header[257:262] != b"ustar":
                raise SetupError("archive_invalid", "the archive is not a ustar archive")
            typeflag = header[156:157]
            if typeflag not in PERMITTED_TYPEFLAGS:
                raise SetupError(
                    "archive_invalid",
                    f"the archive holds an unsupported tar record of type {typeflag!r}; "
                    "this release format holds regular files and one directory only",
                )
            members += 1
            if members > MAX_MEMBERS:
                raise SetupError("archive_limit", f"the archive holds more than {MAX_MEMBERS} entries")
            raw_size = header[124:136].split(b"\x00")[0].split(b" ")[0]
            try:
                size = int(raw_size or b"0", 8)
            except ValueError:
                raise SetupError("archive_invalid", "the archive holds an unreadable member size") from None
            if size < 0:
                raise SetupError("archive_invalid", "the archive holds a negative member size")
            expanded += size
            if expanded > MAX_EXPANDED_BYTES:
                raise SetupError(
                    "archive_limit",
                    f"the archive expands beyond {MAX_EXPANDED_BYTES} bytes",
                )
            skip = (size + TAR_BLOCK - 1) // TAR_BLOCK * TAR_BLOCK
            if skip:
                handle.seek(skip, os.SEEK_CUR)
    if members == 0:
        raise SetupError("archive_invalid", "the archive holds no member")


def prepare_archive(archive_path: str, work: str, deadline: float) -> str:
    """Expand and pre-validate one downloaded archive; return the tar path.

    Both bounds run before any tar library sees the bytes, so an oversized
    extension header can neither allocate nor expand inside that library.
    """
    tar_path = os.path.join(work, "release.tar")
    _bounded_gunzip(archive_path, tar_path, deadline)
    _prescan_tar(tar_path, deadline)
    return tar_path


def _refuse_name(name: str) -> None:
    if not name:
        raise SetupError("archive_invalid", "the archive holds an entry with an empty name")
    if name.startswith("/") or name.startswith("\\"):
        raise SetupError("archive_invalid", f"the archive holds an absolute entry {name!r}")
    if "\\" in name:
        raise SetupError("archive_invalid", f"the archive entry {name!r} holds a backslash")
    if any(ord(character) < 0x20 or ord(character) == 0x7F for character in name):
        raise SetupError("archive_invalid", f"the archive entry {name!r} holds a control character")
    parts = name.split("/")
    if any(part in ("", ".", "..") for part in parts):
        raise SetupError("archive_invalid", f"the archive entry {name!r} is not a plain relative path")


def inspect_archive(tar_path: str, contract: ArchiveContract, deadline: float) -> tarfile.TarInfo:
    """Inspect every member and return the executable member.

    The input is the expanded tar that [`prepare_archive`] already bounded and
    cleared of extended metadata. The function never calls `extractall` and
    never runs `tar`. It rejects a link, a device, a FIFO, a sparse entry, a
    duplicate path, an unexpected name and any bound violation before it reads
    member content.
    """
    seen: set[str] = set()
    expanded = 0
    binary: tarfile.TarInfo | None = None
    permitted = contract.permitted()
    with tarfile.open(tar_path, "r:") as archive:
        for index, member in enumerate(archive):
            if time.monotonic() > deadline:
                raise SetupError("archive_limit", "archive inspection exceeded its time bound")
            if index >= MAX_MEMBERS:
                raise SetupError(
                    "archive_limit",
                    f"the archive holds more than {MAX_MEMBERS} entries",
                )
            name = member.name.rstrip("/")
            _refuse_name(name)
            if name in seen:
                raise SetupError("archive_invalid", f"the archive holds the duplicate entry {name!r}")
            seen.add(name)
            if name not in permitted:
                raise SetupError(
                    "archive_invalid",
                    f"the archive holds the unexpected entry {name!r}; "
                    f"this release contract holds {sorted(permitted)}",
                )
            if member.issym() or member.islnk():
                raise SetupError("archive_invalid", f"the archive entry {name!r} is a link")
            if member.ischr() or member.isblk() or member.isfifo() or member.isdev():
                raise SetupError("archive_invalid", f"the archive entry {name!r} is a special file")
            if getattr(member, "sparse", None) is not None:
                raise SetupError("archive_invalid", f"the archive entry {name!r} is sparse")
            if name == contract.prefix:
                if not member.isdir():
                    raise SetupError(
                        "archive_invalid",
                        f"the archive entry {name!r} must be the release directory",
                    )
                continue
            if not member.isreg():
                raise SetupError("archive_invalid", f"the archive entry {name!r} is not a regular file")
            if member.size < 0:
                raise SetupError("archive_invalid", f"the archive entry {name!r} has a negative size")
            expanded += member.size
            if expanded > MAX_EXPANDED_BYTES:
                raise SetupError(
                    "archive_limit",
                    f"the archive expands beyond {MAX_EXPANDED_BYTES} bytes",
                )
            if name == contract.binary:
                binary = member
        missing = sorted(permitted - seen - {contract.prefix})
        if missing:
            raise SetupError("archive_invalid", f"the archive is missing {missing}")
        if binary is None:
            raise SetupError("archive_invalid", f"the archive is missing {contract.binary!r}")
    return binary


def extract_binary(tar_path: str, member: tarfile.TarInfo, destination: str, deadline: float) -> None:
    """Copy one inspected member into `destination` with explicit mode 0755.

    The copy never inherits the archive's ownership, mode or timestamps.
    """
    with tarfile.open(tar_path, "r:") as archive:
        source = archive.extractfile(member)
        if source is None:
            raise SetupError("archive_invalid", f"the archive entry {member.name!r} has no content")
        written = 0
        descriptor = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o755)
        with os.fdopen(descriptor, "wb") as target:
            while True:
                if time.monotonic() > deadline:
                    raise SetupError("archive_limit", "archive expansion exceeded its time bound")
                chunk = source.read(65536)
                if not chunk:
                    break
                written += len(chunk)
                if written > MAX_EXPANDED_BYTES or written > member.size:
                    raise SetupError("archive_limit", "the archive entry is larger than it declared")
                target.write(chunk)
            target.flush()
            os.fsync(target.fileno())
    os.chmod(destination, 0o755)
    mode = stat.S_IMODE(os.stat(destination).st_mode)
    if mode != 0o755:
        raise SetupError("binary_invalid", f"the installed executable has mode {mode:o}, not 755")


# ---------------------------------------------------------------------------
# Transport
# ---------------------------------------------------------------------------


def _curl_binary(env: Mapping[str, str]) -> str:
    found = shutil.which("curl", path=env.get("PATH", os.defpath))
    if found is None:
        raise SetupError("prerequisite_missing", "curl is not available on this runner")
    return found


def _transient(code: int) -> bool:
    return code == 429 or 500 <= code <= 599


def download(env: Mapping[str, str], url: str, destination: str, limit: int, what: str) -> bytes:
    """Download one HTTPS resource into `destination` and return its bytes.

    The transfer keeps HTTPS across redirects, bounds the connection and the
    whole attempt, and bounds the stored bytes. A transient transport error, a
    429 response and a 5xx response are retried. A 404 response and every
    deterministic failure are not retried.
    """
    curl = _curl_binary(env)
    last: SetupError | None = None
    for attempt in range(1, MAX_ATTEMPTS + 1):
        if os.path.exists(destination):
            os.unlink(destination)
        completed = subprocess.run(
            [
                curl,
                "--disable",
                "--silent",
                "--show-error",
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--location",
                "--max-redirs",
                "5",
                "--connect-timeout",
                str(CONNECT_TIMEOUT_SECONDS),
                "--max-time",
                str(MAX_TIME_SECONDS),
                "--max-filesize",
                str(limit),
                "--output",
                destination,
                "--write-out",
                "%{http_code}",
                url,
            ],
            capture_output=True,
            text=True,
            timeout=MAX_TIME_SECONDS + CONNECT_TIMEOUT_SECONDS,
            env={"PATH": env.get("PATH", os.defpath), "HOME": env.get("HOME", "/tmp")},
        )
        status = (completed.stdout or "").strip().splitlines()[-1:] or ["000"]
        try:
            code = int(status[0])
        except ValueError:
            code = 0
        if completed.returncode != 0:
            # The response body is never printed: it is unverified remote text.
            # Only the first line of curl's own diagnostic reaches the log.
            diagnostic = ((completed.stderr or "").strip().splitlines() or ["no diagnostic"])[0]
            last = SetupError(
                "download_failed",
                f"{what}: curl exited {completed.returncode} for {url} ({diagnostic[:200]})",
            )
            if completed.returncode in (6, 7, 28, 35, 52, 55, 56):
                if attempt < MAX_ATTEMPTS:
                    time.sleep(RETRY_DELAY_SECONDS)
                    continue
            raise last
        if code == 404:
            raise SetupError(
                "asset_missing",
                f"{what}: {url} does not exist; publish that exact version and architecture asset",
            )
        if code >= 400:
            last = SetupError("download_failed", f"{what}: HTTP {code} for {url}")
            if _transient(code) and attempt < MAX_ATTEMPTS:
                time.sleep(RETRY_DELAY_SECONDS)
                continue
            raise last
        if not os.path.isfile(destination):
            last = SetupError("download_failed", f"{what}: no content was stored from {url}")
            if attempt < MAX_ATTEMPTS:
                time.sleep(RETRY_DELAY_SECONDS)
                continue
            raise last
        size = os.path.getsize(destination)
        if size > limit:
            raise SetupError(
                "download_limit",
                f"{what}: {url} returned {size} bytes, above the {limit} byte bound",
            )
        if size == 0:
            last = SetupError("download_failed", f"{what}: {url} returned an empty response")
            if attempt < MAX_ATTEMPTS:
                time.sleep(RETRY_DELAY_SECONDS)
                continue
            raise last
        with open(destination, "rb") as handle:
            return handle.read()
    raise last or SetupError("download_failed", f"{what}: {url} could not be downloaded")


def digest_file(path: str) -> str:
    hasher = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(65536), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


# ---------------------------------------------------------------------------
# Runtime verification and publication
# ---------------------------------------------------------------------------


def _read_bounded(stream, limit: int, label: str, results: dict, process) -> None:
    """Read at most `limit` bytes from one child stream.

    The reader stops at the bound and stops the child, so an unbounded
    producer can neither exhaust this process nor block behind a full pipe.

    The reader records its final result exactly once. A caller must treat a
    missing entry as an unfinished read, never as an empty stream.
    """
    collected = bytearray()
    try:
        while True:
            chunk = stream.read(65536)
            if not chunk:
                break
            room = limit - len(collected)
            if len(chunk) > room:
                collected.extend(chunk[:room])
                results[label] = (bytes(collected), True)
                try:
                    process.kill()
                except OSError:
                    pass
                return
            collected.extend(chunk)
    except OSError:
        pass
    finally:
        try:
            stream.close()
        except OSError:
            pass
    results.setdefault(label, (bytes(collected), False))


def check_version(binary: str, expected: str) -> str:
    """Invoke the installed executable and require exact version equality.

    One deadline covers the whole check: the child, and both stream readers.
    Each wait gets the time that remains, never a fresh full deadline, so the
    check cannot run longer than `VERSION_DEADLINE_SECONDS` by waiting on one
    part after another.

    Both streams are bounded while they are read, and both must finish. A
    reader that is still running, or that recorded no final result, is a
    failed validation: a stream whose content is unknown is never treated as
    an empty stream. Success needs both completed stream checks, no overflow,
    a successful child, and the complete matching version response.
    """
    try:
        process = subprocess.Popen(
            [binary, "--version"],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env={"PATH": "/usr/bin:/bin", "HOME": "/tmp"},
        )
    except OSError as error:
        raise SetupError("binary_unusable", f"cannot run the installed executable: {error}") from None

    deadline = time.monotonic() + VERSION_DEADLINE_SECONDS

    def remaining() -> float:
        return max(0.0, deadline - time.monotonic())

    results: dict[str, tuple[bytes, bool]] = {}
    readers = {
        label: threading.Thread(
            target=_read_bounded,
            args=(stream, MAX_VERSION_OUTPUT_BYTES, label, results, process),
            daemon=True,
        )
        for stream, label in ((process.stdout, "stdout"), (process.stderr, "stderr"))
    }
    for reader in readers.values():
        reader.start()

    timed_out = False
    try:
        process.wait(timeout=remaining())
    except subprocess.TimeoutExpired:
        timed_out = True
    for reader in readers.values():
        reader.join(timeout=remaining())

    # Cleanup is bounded: the child is stopped, and a reader that another
    # process keeps blocked is a daemon thread that cannot hold this program.
    unfinished = sorted(label for label, reader in readers.items() if reader.is_alive())
    incomplete = sorted(label for label in readers if label not in results)
    if timed_out or unfinished or incomplete:
        try:
            process.kill()
        except OSError:
            pass
        try:
            process.wait(timeout=1)
        except subprocess.TimeoutExpired:
            pass

    for label in ("stdout", "stderr"):
        recorded = results.get(label)
        if recorded is not None and recorded[1]:
            raise SetupError(
                "binary_unusable",
                f"the installed executable wrote more than {MAX_VERSION_OUTPUT_BYTES} bytes to {label}",
            )
    if timed_out:
        raise SetupError(
            "binary_unusable",
            f"the installed executable did not answer --version inside {VERSION_DEADLINE_SECONDS} seconds",
        )
    if unfinished or incomplete:
        pending = ", ".join(sorted(set(unfinished) | set(incomplete)))
        raise SetupError(
            "binary_unusable",
            f"the installed executable left {pending} unfinished inside {VERSION_DEADLINE_SECONDS} seconds; "
            "Memoria does not accept a version answer while a stream is still open",
        )
    if process.returncode != 0:
        raise SetupError(
            "binary_unusable",
            f"the installed executable exited {process.returncode} for --version",
        )
    reported = results["stdout"][0].decode("utf-8", "replace").strip()
    if reported != f"memoria {expected}":
        raise SetupError(
            "version_mismatch",
            f"the installed executable reports {reported!r}, not 'memoria {expected}'",
        )
    return reported


def annotate(stream, level: str, message: str) -> None:
    """Write one workflow annotation with control characters escaped."""
    escaped = message.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")
    print(f"::{level}::{escaped}", file=stream)


def publish(env: Mapping[str, str], directory: str, binary: str, version: str) -> None:
    """Append the install directory to `PATH` and write the step outputs."""
    if "\n" in directory or "\n" in binary:
        raise SetupError("runner_incomplete", "the install path holds a line break")
    path_file = env["GITHUB_PATH"]
    output_file = env["GITHUB_OUTPUT"]
    try:
        with open(path_file, "a", encoding="utf-8") as handle:
            handle.write(directory + "\n")
    except OSError as error:
        raise SetupError("runner_incomplete", f"cannot write GITHUB_PATH: {error}") from None
    try:
        with open(output_file, "a", encoding="utf-8") as handle:
            handle.write(f"version={version}\n")
            handle.write(f"path={binary}\n")
    except OSError as error:
        raise SetupError("runner_incomplete", f"cannot write GITHUB_OUTPUT: {error}") from None


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------


def install(env: Mapping[str, str], out=None, err=None) -> int:
    out = out if out is not None else sys.stdout
    err = err if err is not None else sys.stderr

    version = normalize_version(env.get("INPUT_VERSION", ""))
    caller_digest = normalize_digest(env.get("INPUT_SHA256"))
    warning = check_platform(sys.platform, read_os_release())
    if warning:
        annotate(err, "warning", warning)
    target = select_target(process_machine(), env.get("RUNNER_ARCH"))

    check_command_file("GITHUB_PATH", env.get("GITHUB_PATH"))
    check_command_file("GITHUB_OUTPUT", env.get("GITHUB_OUTPUT"))
    runner_temp = env.get("RUNNER_TEMP")
    if not runner_temp or not os.path.isdir(runner_temp):
        raise SetupError("runner_incomplete", "RUNNER_TEMP is not set to an existing directory")
    if not os.access(runner_temp, os.W_OK):
        raise SetupError("runner_incomplete", f"RUNNER_TEMP {runner_temp!r} is not writable")
    _curl_binary(env)

    name = archive_name(version, target)
    url = archive_url(version, target)
    print(f"memoria: installing {version} for {target}", file=out)
    print(f"memoria: release asset {url}", file=out)

    work = tempfile.mkdtemp(prefix="memoria-setup-", dir=runner_temp)
    archive_path = os.path.join(work, name)
    sidecar_path = f"{archive_path}.sha256"

    sidecar_bytes = download(env, f"{url}.sha256", sidecar_path, MAX_SIDECAR_BYTES, "checksum sidecar")
    expected = parse_sidecar(sidecar_bytes.decode("utf-8", "replace"), name)
    if caller_digest is not None and caller_digest != expected:
        raise SetupError(
            "digest_mismatch",
            f"the sha256 input does not match the published sidecar for {name}; "
            "a caller matrix must pin one digest for each architecture",
        )

    download(env, url, archive_path, MAX_ARCHIVE_BYTES, "release archive")
    actual = digest_file(archive_path)
    if actual != expected:
        raise SetupError(
            "digest_mismatch",
            f"the downloaded archive digest {actual} does not match the published {expected}",
        )
    if caller_digest is not None and actual != caller_digest:
        raise SetupError("digest_mismatch", "the downloaded archive does not match the sha256 input")

    deadline = time.monotonic() + ARCHIVE_DEADLINE_SECONDS
    contract = ArchiveContract.of(version, target)
    tar_path = prepare_archive(archive_path, work, deadline)
    member = inspect_archive(tar_path, contract, deadline)

    install_dir = tempfile.mkdtemp(prefix="memoria-bin-", dir=runner_temp)
    os.chmod(install_dir, 0o755)
    binary = os.path.join(install_dir, "memoria")
    extract_binary(tar_path, member, binary, deadline)

    with open(binary, "rb") as handle:
        machine = elf_machine(handle.read(64))
    if machine != ELF_MACHINES[target]:
        raise SetupError(
            "binary_invalid",
            f"the installed executable reports ELF machine {machine:#x}, "
            f"not {ELF_MACHINES[target]:#x} for {target}",
        )

    reported = check_version(binary, version)
    publish(env, install_dir, binary, version)
    print(f"memoria: {reported} is installed at {binary}", file=out)
    print(f"memoria: {install_dir} is on PATH for later steps", file=out)
    return 0


def main(env: Mapping[str, str] | None = None, out=None, err=None) -> int:
    env = env if env is not None else os.environ
    err = err if err is not None else sys.stderr
    try:
        return install(env, out=out, err=err)
    except SetupError as error:
        annotate(err, "error", f"{error.code}: {error.message}")
        print(f"memoria setup failed ({error.code}): {error.message}", file=err)
        return 1


if __name__ == "__main__":  # pragma: no cover - process entry point
    raise SystemExit(main())
