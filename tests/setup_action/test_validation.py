"""Input, platform, architecture and sidecar validation."""

from __future__ import annotations

import os
import tempfile
import unittest

from support import setup_memoria as sm

SetupError = sm.SetupError


class VersionTests(unittest.TestCase):
    def test_accepts_exact_stable_versions(self) -> None:
        self.assertEqual(sm.normalize_version("0.5.0"), "0.5.0")
        self.assertEqual(sm.normalize_version("v0.5.0"), "0.5.0")
        self.assertEqual(sm.normalize_version(" 1.10.3 "), "1.10.3")

    def test_rejects_ranges_prereleases_and_latest(self) -> None:
        for raw in ["latest", "^0.5.0", "0.5", "0.5.0-rc.1", "0.5.0+build", "0.05.0", ""]:
            with self.subTest(raw=raw):
                with self.assertRaises(SetupError) as caught:
                    sm.normalize_version(raw)
                self.assertEqual(caught.exception.code, "version_invalid")

    def test_rejects_versions_below_the_floor(self) -> None:
        # The floor is the first version whose release carries both Linux
        # archive pairs. Every earlier version, 0.4.1 included, is refused.
        for raw in ["0.4.1", "0.4.0", "0.3.0", "0.0.1"]:
            with self.subTest(raw=raw):
                with self.assertRaises(SetupError) as caught:
                    sm.normalize_version(raw)
                self.assertEqual(caught.exception.code, "version_unsupported")


class DigestTests(unittest.TestCase):
    def test_optional_digest_is_normalized(self) -> None:
        self.assertIsNone(sm.normalize_digest(None))
        self.assertIsNone(sm.normalize_digest("   "))
        self.assertEqual(sm.normalize_digest("AB" * 32), "ab" * 32)

    def test_rejects_malformed_digests(self) -> None:
        for raw in ["abc", "z" * 64, "ab" * 31]:
            with self.subTest(raw=raw):
                with self.assertRaises(SetupError) as caught:
                    sm.normalize_digest(raw)
                self.assertEqual(caught.exception.code, "digest_invalid")


class PlatformTests(unittest.TestCase):
    def test_accepts_only_the_supported_release(self) -> None:
        self.assertIsNone(sm.check_platform("linux", 'ID=ubuntu\nVERSION_ID="24.04"\n'))
        self.assertIsNone(sm.check_platform("linux", 'ID=ubuntu\nVERSION_ID="24.04.1"\n'))

    def test_refuses_a_newer_release_instead_of_warning(self) -> None:
        # A newer image has no validation behind it, and `ubuntu-latest` can
        # move. The gate widens only after someone validates the new image.
        for release in ("25.04", "26.04", "30.10"):
            with self.subTest(release=release):
                with self.assertRaises(SetupError) as caught:
                    sm.check_platform("linux", f'ID=ubuntu\nVERSION_ID="{release}"\n')
                self.assertEqual(caught.exception.code, "platform_unsupported")
                self.assertIn("24.04", caught.exception.message)

    def test_claims_no_validation_it_does_not_have(self) -> None:
        # The old wording said the Action "was validated on the floor release".
        # Hosted evidence is still deferred, so no message may claim it.
        with self.assertRaises(SetupError) as caught:
            sm.check_platform("linux", 'ID=ubuntu\nVERSION_ID="26.04"\n')
        self.assertNotIn("was validated", caught.exception.message)

    def test_rejects_other_systems_and_older_ubuntu(self) -> None:
        cases = [
            ("darwin", 'ID=ubuntu\nVERSION_ID="24.04"\n'),
            ("win32", 'ID=ubuntu\nVERSION_ID="24.04"\n'),
            ("linux", 'ID=debian\nVERSION_ID="12"\n'),
            ("linux", 'ID=ubuntu\nVERSION_ID="22.04"\n'),
            ("linux", 'ID=ubuntu\nVERSION_ID="not-a-release"\n'),
            ("linux", "ID=ubuntu\n"),
        ]
        for platform, release in cases:
            with self.subTest(platform=platform, release=release):
                with self.assertRaises(SetupError) as caught:
                    sm.check_platform(platform, release)
                self.assertEqual(caught.exception.code, "platform_unsupported")


class TargetTests(unittest.TestCase):
    def test_maps_both_supported_architectures(self) -> None:
        self.assertEqual(sm.select_target("x86_64", "X64"), "x86_64-unknown-linux-gnu")
        self.assertEqual(sm.select_target("aarch64", "ARM64"), "aarch64-unknown-linux-gnu")

    def test_process_architecture_selects_without_a_runner_label(self) -> None:
        self.assertEqual(sm.select_target("aarch64", None), "aarch64-unknown-linux-gnu")
        self.assertEqual(sm.select_target("aarch64", ""), "aarch64-unknown-linux-gnu")

    def test_rejects_a_runner_label_that_disagrees(self) -> None:
        with self.assertRaises(SetupError) as caught:
            sm.select_target("aarch64", "X64")
        self.assertEqual(caught.exception.code, "architecture_mismatch")
        with self.assertRaises(SetupError) as caught:
            sm.select_target("x86_64", "ARM64")
        self.assertEqual(caught.exception.code, "architecture_mismatch")

    def test_rejects_unsupported_architectures(self) -> None:
        for machine in ["armv7l", "i686", "riscv64", "ppc64le"]:
            with self.subTest(machine=machine):
                with self.assertRaises(SetupError) as caught:
                    sm.select_target(machine, None)
                self.assertEqual(caught.exception.code, "architecture_unsupported")

    def test_asset_names_carry_the_architecture(self) -> None:
        self.assertEqual(
            sm.archive_url("0.5.0", "aarch64-unknown-linux-gnu"),
            "https://github.com/viktordanov/rs-memoria/releases/download/v0.5.0/"
            "memoria-0.5.0-aarch64-unknown-linux-gnu.tar.gz",
        )
        self.assertEqual(
            sm.archive_name("0.5.0", "x86_64-unknown-linux-gnu"),
            "memoria-0.5.0-x86_64-unknown-linux-gnu.tar.gz",
        )


class SidecarTests(unittest.TestCase):
    NAME = "memoria-0.5.0-x86_64-unknown-linux-gnu.tar.gz"
    DIGEST = "a" * 64

    def test_accepts_one_matching_record(self) -> None:
        self.assertEqual(sm.parse_sidecar(f"{self.DIGEST}  {self.NAME}\n", self.NAME), self.DIGEST)
        self.assertEqual(sm.parse_sidecar(f"{self.DIGEST} *{self.NAME}\n", self.NAME), self.DIGEST)

    def test_rejects_a_sidecar_for_another_architecture(self) -> None:
        other = "memoria-0.5.0-aarch64-unknown-linux-gnu.tar.gz"
        with self.assertRaises(SetupError) as caught:
            sm.parse_sidecar(f"{self.DIGEST}  {other}\n", self.NAME)
        self.assertEqual(caught.exception.code, "sidecar_mismatch")

    def test_rejects_malformed_and_multi_record_sidecars(self) -> None:
        for text in ["", "not a checksum\n", f"{self.DIGEST}\n", f"zz  {self.NAME}\n",
                     f"{self.DIGEST}  {self.NAME}\n{self.DIGEST}  {self.NAME}\n"]:
            with self.subTest(text=text):
                with self.assertRaises(SetupError) as caught:
                    sm.parse_sidecar(text, self.NAME)
                self.assertIn(caught.exception.code, {"sidecar_invalid", "sidecar_mismatch"})


class ElfTests(unittest.TestCase):
    def header(self, machine: int, klass: int = 2, data: int = 1, magic: bytes = b"\x7fELF") -> bytes:
        header = bytearray(64)
        header[0:4] = magic
        header[4] = klass
        header[5] = data
        header[18:20] = machine.to_bytes(2, "little")
        return bytes(header)

    def test_reads_both_supported_machines(self) -> None:
        self.assertEqual(sm.elf_machine(self.header(0x3E)), 0x3E)
        self.assertEqual(sm.elf_machine(self.header(0xB7)), 0xB7)

    def test_rejects_non_elf_and_wrong_class(self) -> None:
        for header in [b"#!/bin/sh\n" + bytes(54), self.header(0x3E, klass=1),
                       self.header(0x3E, data=2), b"\x7fELF"]:
            with self.subTest(header=header[:8]):
                with self.assertRaises(SetupError) as caught:
                    sm.elf_machine(header)
                self.assertEqual(caught.exception.code, "binary_invalid")


class CommandFileTests(unittest.TestCase):
    def test_requires_an_existing_writable_command_file(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = os.path.join(directory, "path-file")
            self.assertEqual(sm.check_command_file("GITHUB_PATH", path), path)
            self.assertTrue(os.path.exists(path))

    def test_rejects_missing_and_newline_bearing_paths(self) -> None:
        for value in [None, "", "/does/not/exist/file", "/tmp/with\nnewline"]:
            with self.subTest(value=value):
                with self.assertRaises(SetupError) as caught:
                    sm.check_command_file("GITHUB_OUTPUT", value)
                self.assertEqual(caught.exception.code, "runner_incomplete")


if __name__ == "__main__":
    unittest.main()
