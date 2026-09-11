"""Archive inspection: contract members, hostile entries, and bounds.

Every fixture here carries a valid checksum sidecar, so the test reaches
archive validation instead of stopping at the integrity comparison.
"""

from __future__ import annotations

import gzip
import io
import os
import tarfile
import tempfile
import time
import unittest

import support
from support import setup_memoria as sm

VERSION = "0.5.0"
TARGET = support.host_target()
SetupError = sm.SetupError


class Zeros(io.RawIOBase):
    """A reader of `total` zero bytes, so an oversized member costs no disk."""

    def __init__(self, total: int) -> None:
        self.remaining = total

    def readable(self) -> bool:
        return True

    def readinto(self, buffer) -> int:
        count = min(len(buffer), self.remaining)
        buffer[:count] = b"\0" * count
        self.remaining -= count
        return count


def contract() -> sm.ArchiveContract:
    return sm.ArchiveContract.of(VERSION, TARGET)


def deadline() -> float:
    return time.monotonic() + sm.ARCHIVE_DEADLINE_SECONDS


class ArchiveContractTests(unittest.TestCase):
    def setUp(self) -> None:
        self.directory = support.scratch("memoria-archive-")

    def pack(self, **kwargs) -> str:
        return support.pack_archive(self.directory, VERSION, TARGET, **kwargs)

    def refuses(self, path: str, *codes: str) -> sm.SetupError:
        with self.assertRaises(SetupError) as caught:
            sm.inspect_archive(support.prepare(path), contract(), deadline())
        if codes:
            self.assertIn(caught.exception.code, set(codes))
        return caught.exception

    def test_accepts_the_release_contract(self) -> None:
        member = sm.inspect_archive(support.prepare(self.pack()), contract(), deadline())
        self.assertEqual(member.name, f"memoria-{VERSION}-{TARGET}/memoria")

    def test_valid_checksum_does_not_excuse_a_hostile_member(self) -> None:
        path = self.pack(extra=[(f"memoria-{VERSION}-{TARGET}/payload.sh", b"rm -rf /\n")])
        self.assertEqual(support.file_digest(path), support.file_digest(path))
        self.refuses(path, "archive_invalid")

    def test_rejects_traversal_and_absolute_entries(self) -> None:
        self.refuses(self.pack(traversal=True), "archive_invalid")
        self.refuses(self.pack(absolute=True), "archive_invalid")

    def test_rejects_links_and_special_files(self) -> None:
        prefix = f"memoria-{VERSION}-{TARGET}"
        self.refuses(self.pack(links=[(f"{prefix}/link", "/etc/passwd")]), "archive_invalid")
        self.refuses(self.pack(device=True), "archive_invalid")

    def test_rejects_a_symlink_that_replaces_the_executable(self) -> None:
        path = self.pack(omit_binary=True, links=[(f"memoria-{VERSION}-{TARGET}/memoria", "/bin/sh")])
        self.refuses(path, "archive_invalid")

    def test_rejects_duplicate_and_missing_entries(self) -> None:
        self.refuses(self.pack(duplicate=True), "archive_invalid")
        self.refuses(self.pack(omit_binary=True), "archive_invalid")
        self.refuses(self.pack(omit_license=True), "archive_invalid")

    def test_rejects_an_archive_for_another_version_or_architecture(self) -> None:
        other_version = support.pack_archive(self.directory, "0.4.2", TARGET)
        self.refuses(other_version, "archive_invalid")
        other = "aarch64" if TARGET.startswith("x86_64") else "x86_64"
        other_target = support.pack_archive(
            support.scratch("memoria-other-"), VERSION, f"{other}-unknown-linux-gnu"
        )
        self.refuses(other_target, "archive_invalid")

    def test_rejects_a_control_character_in_a_name(self) -> None:
        path = self.pack(extra=[(f"memoria-{VERSION}-{TARGET}/bad\x01name", b"x")])
        self.refuses(path, "archive_invalid")

    def test_rejects_more_members_than_the_bound(self) -> None:
        prefix = f"memoria-{VERSION}-{TARGET}"
        extra = [(f"{prefix}/file{index}", b"x") for index in range(sm.MAX_MEMBERS + 4)]
        self.refuses(self.pack(extra=extra), "archive_limit", "archive_invalid")

    def test_rejects_an_expansion_above_the_bound(self) -> None:
        path = os.path.join(self.directory, f"memoria-{VERSION}-{TARGET}.tar.gz")
        prefix = f"memoria-{VERSION}-{TARGET}"
        oversized = sm.MAX_EXPANDED_BYTES + 1
        with tarfile.open(path, "w:gz") as archive:
            info = tarfile.TarInfo(prefix)
            info.type = tarfile.DIRTYPE
            archive.addfile(info)
            # The declared sizes alone reach the expansion bound, so the
            # refusal happens before the installer copies any member content.
            info = tarfile.TarInfo(f"{prefix}/LICENSE")
            info.size = oversized
            archive.addfile(info, Zeros(oversized))
            info = tarfile.TarInfo(f"{prefix}/memoria")
            info.size = 1
            archive.addfile(info, io.BytesIO(b"x"))
        exception = self.refuses(path, "archive_limit")
        self.assertIn("expands beyond", exception.message)

    def test_expired_deadline_stops_inspection(self) -> None:
        tar_path = support.prepare(self.pack())
        with self.assertRaises(SetupError) as caught:
            sm.inspect_archive(tar_path, contract(), time.monotonic() - 1)
        self.assertEqual(caught.exception.code, "archive_limit")

    def test_expired_deadline_stops_preparation(self) -> None:
        path = self.pack()
        with self.assertRaises(SetupError) as caught:
            support.prepare(path, deadline=time.monotonic() - 1)
        self.assertEqual(caught.exception.code, "archive_limit")


class MetadataBoundTests(unittest.TestCase):
    """The bounds must hold before a tar library reads any metadata."""

    def setUp(self) -> None:
        self.directory = support.scratch("memoria-metadata-")

    def test_an_oversized_extension_header_never_reaches_the_tar_library(self) -> None:
        # The compressed file is tiny and its checksum is valid, so the fixture
        # reaches archive handling. Python's tarfile expands a PAX header
        # internally, which the installer must prevent.
        path = support.pack_pax_bomb(self.directory, VERSION, TARGET, 65 * 1024 * 1024)
        self.assertLess(os.path.getsize(path), 1024 * 1024)
        with self.assertRaises(SetupError) as caught:
            support.prepare(path)
        self.assertIn(caught.exception.code, {"archive_limit", "archive_invalid"})

    def test_every_extended_metadata_record_is_refused(self) -> None:
        prefix = f"memoria-{VERSION}-{TARGET}"
        for type_flag, label in (
            (tarfile.XHDTYPE, "pax extended"),
            (tarfile.XGLTYPE, "pax global"),
            (tarfile.GNUTYPE_LONGNAME, "gnu long name"),
            (tarfile.GNUTYPE_LONGLINK, "gnu long link"),
        ):
            with self.subTest(label=label):
                path = os.path.join(self.directory, f"{label.replace(' ', '-')}.tar.gz")
                record = b"20 comment=short\n"
                with tarfile.open(path, "w:gz") as archive:
                    info = tarfile.TarInfo(f"{prefix}/PaxHeaders/x")
                    info.type = type_flag
                    info.size = len(record)
                    archive.addfile(info, io.BytesIO(record))
                    member = tarfile.TarInfo(f"{prefix}/memoria")
                    member.size = 1
                    archive.addfile(member, io.BytesIO(b"x"))
                with self.assertRaises(SetupError) as caught:
                    support.prepare(path)
                self.assertEqual(caught.exception.code, "archive_invalid")

    def test_a_gzip_bomb_stops_at_the_expansion_bound(self) -> None:
        path = os.path.join(self.directory, "bomb.tar.gz")
        with gzip.open(path, "wb") as handle:
            written = 0
            chunk = bytes(1024 * 1024)
            while written <= sm.MAX_EXPANDED_BYTES:
                handle.write(chunk)
                written += len(chunk)
        self.assertLess(os.path.getsize(path), 1024 * 1024)
        with self.assertRaises(SetupError) as caught:
            support.prepare(path)
        self.assertEqual(caught.exception.code, "archive_limit")

    def test_the_produced_release_archive_still_prepares(self) -> None:
        path = support.pack_archive(self.directory, VERSION, TARGET)
        member = sm.inspect_archive(support.prepare(path), contract(), deadline())
        self.assertEqual(member.name, f"memoria-{VERSION}-{TARGET}/memoria")


class ExtractionTests(unittest.TestCase):
    def setUp(self) -> None:
        self.directory = support.scratch("memoria-extract-")
        self.archive = support.prepare(support.pack_archive(self.directory, VERSION, TARGET))
        self.member = sm.inspect_archive(self.archive, contract(), deadline())

    def test_copies_only_the_executable_with_explicit_mode(self) -> None:
        destination = os.path.join(self.directory, "install", "memoria")
        os.makedirs(os.path.dirname(destination))
        sm.extract_binary(self.archive, self.member, destination, deadline())
        self.assertEqual(os.listdir(os.path.dirname(destination)), ["memoria"])
        self.assertEqual(os.stat(destination).st_mode & 0o777, 0o755)

    def test_never_clobbers_an_existing_destination(self) -> None:
        destination = os.path.join(self.directory, "taken")
        with open(destination, "wb") as handle:
            handle.write(b"existing")
        with self.assertRaises(FileExistsError):
            sm.extract_binary(self.archive, self.member, destination, deadline())
        with open(destination, "rb") as handle:
            self.assertEqual(handle.read(), b"existing")

    def test_reports_the_elf_machine_of_the_installed_file(self) -> None:
        destination = os.path.join(self.directory, "memoria")
        sm.extract_binary(self.archive, self.member, destination, deadline())
        with open(destination, "rb") as handle:
            machine = sm.elf_machine(handle.read(64))
        self.assertEqual(machine, sm.ELF_MACHINES[TARGET])


if __name__ == "__main__":
    unittest.main()
