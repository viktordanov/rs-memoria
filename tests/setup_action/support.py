"""Shared helpers for the setup Action fixtures.

The helpers load `scripts/setup-memoria.py` as a module, build real ELF
executables for the host architecture, and write a fake `curl` that serves a
fixed routing table and records the exact requested URLs.

No fixture reaches the network. No fixture writes inside the repository.
"""

from __future__ import annotations

import atexit
import hashlib
import importlib.util
import io
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import textwrap
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPT = REPO_ROOT / "scripts" / "setup-memoria.py"
ACTION = REPO_ROOT / "action.yml"

#: The Ubuntu release description the installer accepts.
UBUNTU_2404 = 'PRETTY_NAME="Ubuntu 24.04.1 LTS"\nNAME="Ubuntu"\nID=ubuntu\nVERSION_ID="24.04"\n'


def load_module():
    """Import `scripts/setup-memoria.py` under a valid module name."""
    spec = importlib.util.spec_from_file_location("setup_memoria", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    sys.modules["setup_memoria"] = module
    spec.loader.exec_module(module)
    return module


setup_memoria = load_module()


def host_target() -> str:
    """The release target this host would select."""
    return setup_memoria.TARGETS[os.uname().machine]


_STUB_CACHE: dict[tuple[str, str], str] = {}


def build_stub(version: str, behavior: str = "version") -> str:
    """Compile a real ELF executable that imitates one `memoria` binary.

    `behavior` selects `version` (prints `memoria <version>`), `wrong`
    (prints another version), `fail` (exits 3) or `hang` (sleeps past the
    five-second deadline). The executable is a real ELF for this host, not a
    shell script, so the ELF machine check inspects actual bytes.
    """
    key = (version, behavior)
    cached = _STUB_CACHE.get(key)
    if cached and os.path.exists(cached):
        return cached
    body = {
        "version": f'println!("memoria {version}");',
        "wrong": 'println!("memoria 9.9.9");',
        "fail": "std::process::exit(3);",
        "hang": "std::thread::sleep(std::time::Duration::from_secs(30));",
        # The expected answer, then padding, then text that must make the
        # whole response a mismatch instead of an accepted prefix.
        # Inside the output bound, so the refusal proves the comparison uses
        # the whole permitted response instead of a truncated prefix.
        "trailing": (
            'use std::io::Write; let mut o = std::io::stdout(); '
            f'write!(o, "memoria {version}").unwrap(); '
            'write!(o, "{}", " ".repeat(100)).unwrap(); '
            'write!(o, "WRONG TRAILING OUTPUT").unwrap(); o.flush().unwrap();'
        ),
        "overflow_stdout": (
            'use std::io::Write; let mut o = std::io::stdout(); '
            f'write!(o, "memoria {version}\\n").unwrap(); '
            'for _ in 0..4096 { write!(o, "{}", "x".repeat(4096)).unwrap(); } o.flush().unwrap();'
        ),
        # The parent answers correctly and exits, but a grandchild inherits the
        # stderr pipe and keeps it open. The stdout reader sees end of file
        # while the stderr reader is still blocked, which is the state that
        # must never be accepted as a validated empty stream.
        "held_stderr": (
            'use std::process::{Command, Stdio}; '
            'let _ = Command::new("/bin/sh").args(["-c", '
            '"sleep 7; dd if=/dev/zero bs=5000 count=1 2>/dev/null | tr \'\\\\0\' x >&2"]) '
            '.stdout(Stdio::null()).spawn(); '
            f'println!("memoria {version}");'
        ),
        # Both pipes stay open in a grandchild while the parent itself never
        # exits. One overall deadline must cover the wait and both joins; a
        # fresh deadline for each of the three would take three times as long.
        "held_streams_hang": (
            'use std::process::{Command, Stdio}; '
            'let _ = Command::new("/bin/sh").args(["-c", "sleep 12"]) '
            '.stdout(Stdio::inherit()).stderr(Stdio::inherit()).spawn(); '
            'std::thread::sleep(std::time::Duration::from_secs(12));'
        ),
        "overflow_stderr": (
            'use std::io::Write; '
            f'println!("memoria {version}"); '
            'let mut e = std::io::stderr(); '
            'for _ in 0..4096 { write!(e, "{}", "y".repeat(4096)).unwrap(); } e.flush().unwrap();'
        ),
    }[behavior]
    directory = scratch("memoria-stub-")
    source = Path(directory) / "stub.rs"
    source.write_text(f"fn main() {{ {body} }}\n", encoding="utf-8")
    output = Path(directory) / "memoria"
    rustc = shutil.which("rustc")
    if rustc is None:  # pragma: no cover - the repository toolchain provides it
        raise RuntimeError("rustc is required to build the Action fixtures")
    subprocess.run(
        [rustc, "--edition", "2021", "-C", "opt-level=0", "-o", str(output), str(source)],
        check=True,
        capture_output=True,
    )
    _STUB_CACHE[key] = str(output)
    return str(output)


def pack_archive(
    directory: str,
    version: str,
    target: str,
    *,
    binary: str | None = None,
    extra: list[tuple[str, bytes]] | None = None,
    links: list[tuple[str, str]] | None = None,
    absolute: bool = False,
    traversal: bool = False,
    duplicate: bool = False,
    omit_license: bool = False,
    omit_binary: bool = False,
    device: bool = False,
    binary_bytes: bytes | None = None,
) -> str:
    """Write one release archive and its sidecar; return the archive path.

    Every deviation from the release contract is explicit, so a fixture can
    reach archive validation with a valid checksum.
    """
    prefix = f"memoria-{version}-{target}"
    path = os.path.join(directory, f"memoria-{version}-{target}.tar.gz")
    with tarfile.open(path, "w:gz") as archive:
        info = tarfile.TarInfo(prefix)
        info.type = tarfile.DIRTYPE
        info.mode = 0o755
        archive.addfile(info)
        if not omit_license:
            data = b"MIT\n"
            info = tarfile.TarInfo(f"{prefix}/LICENSE")
            info.size = len(data)
            info.mode = 0o644
            archive.addfile(info, io.BytesIO(data))
        if not omit_binary:
            if binary_bytes is None:
                with open(binary or build_stub(version), "rb") as handle:
                    binary_bytes = handle.read()
            info = tarfile.TarInfo(f"{prefix}/memoria")
            info.size = len(binary_bytes)
            info.mode = 0o755
            archive.addfile(info, io.BytesIO(binary_bytes))
            if duplicate:
                info = tarfile.TarInfo(f"{prefix}/memoria")
                info.size = len(binary_bytes)
                info.mode = 0o755
                archive.addfile(info, io.BytesIO(binary_bytes))
        for name, data in extra or []:
            info = tarfile.TarInfo(name)
            info.size = len(data)
            info.mode = 0o644
            archive.addfile(info, io.BytesIO(data))
        for name, destination in links or []:
            info = tarfile.TarInfo(name)
            info.type = tarfile.SYMTYPE
            info.linkname = destination
            archive.addfile(info)
        if absolute:
            info = tarfile.TarInfo(f"/etc/{prefix}")
            info.size = 0
            archive.addfile(info)
        if traversal:
            info = tarfile.TarInfo(f"{prefix}/../escape")
            info.size = 0
            archive.addfile(info)
        if device:
            info = tarfile.TarInfo(f"{prefix}/null")
            info.type = tarfile.CHRTYPE
            info.devmajor = 1
            info.devminor = 3
            archive.addfile(info)
    write_sidecar(path)
    return path


def write_sidecar(archive_path: str, *, name: str | None = None, digest: str | None = None) -> str:
    """Write the `sha256sum` sidecar next to one archive."""
    sidecar = f"{archive_path}.sha256"
    with open(archive_path, "rb") as handle:
        actual = hashlib.sha256(handle.read()).hexdigest()
    with open(sidecar, "w", encoding="utf-8") as handle:
        handle.write(f"{digest or actual}  {name or os.path.basename(archive_path)}\n")
    return sidecar


def scratch(prefix: str) -> str:
    """A temporary directory this process removes when it exits.

    The expanded tar of a bound fixture is large, so fixture storage must not
    survive the run that created it.
    """
    directory = tempfile.mkdtemp(prefix=prefix)
    atexit.register(shutil.rmtree, directory, True)
    return directory


def prepare(archive_path: str, deadline: float | None = None) -> str:
    """Expand and pre-validate one archive, as the installer does."""
    import time as _time

    work = scratch("memoria-prepared-")
    if deadline is None:
        deadline = _time.monotonic() + setup_memoria.ARCHIVE_DEADLINE_SECONDS
    try:
        return setup_memoria.prepare_archive(archive_path, work, deadline)
    except setup_memoria.SetupError:
        # A refused fixture leaves a partial expansion behind. Remove it now
        # rather than at exit, so a bound fixture costs no lasting storage.
        shutil.rmtree(work, ignore_errors=True)
        raise


def pack_pax_bomb(directory: str, version: str, target: str, payload_bytes: int) -> str:
    """Write an archive whose PAX header alone is larger than the bound.

    The compressed file stays small, so the fixture reaches archive handling
    with a valid checksum. A tar library expands that header internally, which
    is exactly what the installer must prevent.
    """
    prefix = f"memoria-{version}-{target}"
    path = os.path.join(directory, f"memoria-{version}-{target}.tar.gz")
    record = b"%d comment=%s\n" % (payload_bytes, b"A" * (payload_bytes - 20))
    header = tarfile.TarInfo(f"{prefix}/PaxHeaders/memoria")
    header.type = tarfile.XHDTYPE
    header.size = len(record)
    with tarfile.open(path, "w:gz") as archive:
        archive.addfile(header, io.BytesIO(record))
        info = tarfile.TarInfo(f"{prefix}/memoria")
        info.size = 1
        info.mode = 0o755
        archive.addfile(info, io.BytesIO(b"x"))
    write_sidecar(path)
    return path


def file_digest(path: str) -> str:
    with open(path, "rb") as handle:
        return hashlib.sha256(handle.read()).hexdigest()


FAKE_CURL = '''#!{python}
"""A fake curl that serves one fixed routing table and records requests."""
import json, os, shutil, sys

ROUTES = json.loads({routes!r})
LOG = {log!r}

argv = sys.argv[1:]
url = argv[-1]
output = None
for index, item in enumerate(argv):
    if item == "--output":
        output = argv[index + 1]

with open(LOG, "a", encoding="utf-8") as handle:
    handle.write(url + "\\n")

route = ROUTES.get(url)
if route is None:
    sys.stderr.write("fake curl: no route for " + url + "\\n")
    print("404")
    raise SystemExit(0)

exit_code = route.get("exit", 0)
if exit_code:
    sys.stderr.write(route.get("stderr", "transport failure") + "\\n")
    raise SystemExit(exit_code)

status = route.get("status", 200)
if status < 400:
    source = route.get("file")
    if source is not None:
        shutil.copyfile(source, output)
    else:
        with open(output, "wb") as handle:
            handle.write(route.get("body", "").encode())
print(status)
raise SystemExit(0)
'''


def write_fake_curl(directory: str, routes: dict, log: str) -> str:
    """Write a fake `curl` into `directory` and return that directory."""
    os.makedirs(directory, exist_ok=True)
    script = os.path.join(directory, "curl")
    with open(script, "w", encoding="utf-8") as handle:
        handle.write(FAKE_CURL.format(python=sys.executable, routes=json.dumps(routes), log=log))
    os.chmod(script, 0o755)
    poison = os.path.join(directory, "cargo")
    with open(poison, "w", encoding="utf-8") as handle:
        handle.write(
            textwrap.dedent(
                """\
                #!/bin/sh
                echo "the setup Action must never invoke cargo" >&2
                touch "$(dirname "$0")/cargo-was-invoked"
                exit 97
                """
            )
        )
    os.chmod(poison, 0o755)
    return directory


def cargo_invoked(directory: str) -> bool:
    return os.path.exists(os.path.join(directory, "cargo-was-invoked"))
