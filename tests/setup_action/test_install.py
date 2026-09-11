"""End-to-end installation, transport failures, and runner preservation.

Each fixture replaces `curl` with a recorded routing table and replaces the
architecture and operating-system seams. No fixture reaches the network.
"""

from __future__ import annotations

import io
import os
import time
import tempfile
import unittest
from pathlib import Path

import support
from support import setup_memoria as sm

VERSION = "0.5.0"
TARGET = support.host_target()
ARCHIVE = f"memoria-{VERSION}-{TARGET}.tar.gz"
URL = sm.archive_url(VERSION, TARGET)


class Runner(unittest.TestCase):
    """A fake runner with a temporary directory and workflow command files."""

    def setUp(self) -> None:
        self.base = support.scratch("memoria-runner-")
        self.temp = os.path.join(self.base, "runner-temp")
        self.tools = os.path.join(self.base, "tools")
        self.project = os.path.join(self.base, "project")
        os.makedirs(self.temp)
        os.makedirs(self.project)
        Path(self.project, "memoria.lock").write_text("untouched\n", encoding="utf-8")
        Path(self.project, "README.md").write_text("# Consumer\n", encoding="utf-8")
        self.path_file = os.path.join(self.base, "github-path")
        self.output_file = os.path.join(self.base, "github-output")
        self.log = os.path.join(self.base, "curl-log")
        Path(self.path_file).write_text("", encoding="utf-8")
        Path(self.output_file).write_text("", encoding="utf-8")
        self.release = support.scratch("memoria-release-")
        self.out = io.StringIO()
        self.err = io.StringIO()
        self._machine = sm.process_machine
        self._release = sm.read_os_release
        sm.read_os_release = lambda: support.UBUNTU_2404
        self.addCleanup(self.restore)

    def restore(self) -> None:
        sm.process_machine = self._machine
        sm.read_os_release = self._release

    # -- fixture construction ------------------------------------------------

    def serve(self, routes: dict) -> None:
        support.write_fake_curl(self.tools, routes, self.log)

    def serve_release(self, **kwargs) -> str:
        archive = support.pack_archive(self.release, VERSION, TARGET, **kwargs)
        self.serve(
            {
                URL: {"file": archive},
                f"{URL}.sha256": {"file": f"{archive}.sha256"},
            }
        )
        return archive

    def env(self, **overrides) -> dict:
        base = {
            "INPUT_VERSION": VERSION,
            "PATH": f"{self.tools}:{os.environ['PATH']}",
            "HOME": self.base,
            "RUNNER_TEMP": self.temp,
            "RUNNER_ARCH": sm.RUNNER_ARCHES[TARGET],
            "GITHUB_PATH": self.path_file,
            "GITHUB_OUTPUT": self.output_file,
        }
        base.update(overrides)
        return {key: value for key, value in base.items() if value is not None}

    def install(self, **overrides) -> int:
        return sm.install(self.env(**overrides), out=self.out, err=self.err)

    def refuse(self, **overrides) -> sm.SetupError:
        with self.assertRaises(sm.SetupError) as caught:
            self.install(**overrides)
        return caught.exception

    # -- observations --------------------------------------------------------

    def outputs(self) -> dict:
        values = {}
        for line in Path(self.output_file).read_text(encoding="utf-8").splitlines():
            key, _, value = line.partition("=")
            values[key] = value
        return values

    def path_entries(self) -> list[str]:
        return [line for line in Path(self.path_file).read_text(encoding="utf-8").splitlines() if line]

    def no_retry_delay(self) -> None:
        """Remove the retry pause so a bounded-retry fixture stays fast."""
        self.addCleanup(setattr, sm, "RETRY_DELAY_SECONDS", sm.RETRY_DELAY_SECONDS)
        sm.RETRY_DELAY_SECONDS = 0

    def requested(self) -> list[str]:
        if not os.path.exists(self.log):
            return []
        return [line for line in Path(self.log).read_text(encoding="utf-8").splitlines() if line]


class SuccessTests(Runner):
    def test_installs_and_publishes_path_and_outputs(self) -> None:
        self.serve_release()
        self.assertEqual(self.install(), 0)
        outputs = self.outputs()
        self.assertEqual(outputs["version"], VERSION)
        binary = outputs["path"]
        self.assertTrue(os.access(binary, os.X_OK))
        self.assertEqual(os.stat(binary).st_mode & 0o777, 0o755)
        self.assertEqual(self.path_entries(), [os.path.dirname(binary)])
        self.assertTrue(binary.startswith(self.temp))

    def test_requests_the_exact_architecture_asset(self) -> None:
        self.serve_release()
        self.install()
        self.assertEqual(self.requested(), [f"{URL}.sha256", URL])
        self.assertIn(TARGET, self.requested()[0])

    def test_accepts_a_leading_v_and_a_matching_caller_digest(self) -> None:
        archive = self.serve_release()
        self.assertEqual(
            self.install(INPUT_VERSION=f"v{VERSION}", INPUT_SHA256=support.file_digest(archive)), 0
        )
        self.assertEqual(self.outputs()["version"], VERSION)

    def test_repeated_installs_use_separate_directories(self) -> None:
        self.serve_release()
        self.install()
        first = self.outputs()["path"]
        self.install()
        second = self.outputs()["path"]
        self.assertNotEqual(first, second)
        self.assertEqual(self.path_entries(), [os.path.dirname(first), os.path.dirname(second)])

    def test_installs_into_a_runner_temp_with_spaces(self) -> None:
        spaced = os.path.join(self.base, "runner temp with spaces")
        os.makedirs(spaced)
        self.serve_release()
        self.assertEqual(self.install(RUNNER_TEMP=spaced), 0)
        self.assertTrue(self.outputs()["path"].startswith(spaced))

    def test_selects_the_asset_from_the_process_architecture(self) -> None:
        self.serve_release()
        self.install(RUNNER_ARCH=None)
        self.assertEqual(self.requested()[-1], URL)


class ArchitectureTests(Runner):
    def test_rejects_a_runner_label_that_disagrees_before_any_request(self) -> None:
        self.serve_release()
        other = "ARM64" if sm.RUNNER_ARCHES[TARGET] == "X64" else "X64"
        error = self.refuse(RUNNER_ARCH=other)
        self.assertEqual(error.code, "architecture_mismatch")
        self.assertEqual(self.requested(), [])

    def test_rejects_an_unsupported_architecture_before_any_request(self) -> None:
        self.serve_release()
        sm.process_machine = lambda: "riscv64"
        error = self.refuse(RUNNER_ARCH=None)
        self.assertEqual(error.code, "architecture_unsupported")
        self.assertEqual(self.requested(), [])

    def test_rejects_a_non_ubuntu_runner_before_any_request(self) -> None:
        self.serve_release()
        sm.read_os_release = lambda: 'ID=arch\nVERSION_ID="rolling"\n'
        error = self.refuse()
        self.assertEqual(error.code, "platform_unsupported")
        self.assertEqual(self.requested(), [])


class TransportTests(Runner):
    def test_missing_release_names_the_version_and_architecture(self) -> None:
        self.serve({URL: {"status": 404}, f"{URL}.sha256": {"status": 404}})
        error = self.refuse()
        self.assertEqual(error.code, "asset_missing")
        self.assertIn(TARGET, error.message)
        self.assertIn(f"v{VERSION}", error.message)
        self.assertEqual(len(self.requested()), 1, "a 404 is never retried")

    def test_missing_archive_with_a_present_sidecar_still_fails(self) -> None:
        archive = support.pack_archive(self.release, VERSION, TARGET)
        self.serve({f"{URL}.sha256": {"file": f"{archive}.sha256"}, URL: {"status": 404}})
        self.assertEqual(self.refuse().code, "asset_missing")

    def test_retries_transient_responses_and_then_fails(self) -> None:
        self.no_retry_delay()
        for status in (429, 500, 503):
            with self.subTest(status=status):
                Path(self.log).write_text("", encoding="utf-8")
                self.serve({f"{URL}.sha256": {"status": status}})
                error = self.refuse()
                self.assertEqual(error.code, "download_failed")
                self.assertEqual(len(self.requested()), sm.MAX_ATTEMPTS)

    def test_does_not_retry_a_forbidden_response(self) -> None:
        self.serve({f"{URL}.sha256": {"status": 403}})
        self.assertEqual(self.refuse().code, "download_failed")
        self.assertEqual(len(self.requested()), 1)

    def test_reports_tls_and_timeout_failures_without_body_text(self) -> None:
        self.no_retry_delay()
        for code, label in ((35, "TLS"), (28, "timeout")):
            with self.subTest(label=label):
                Path(self.log).write_text("", encoding="utf-8")
                self.serve({f"{URL}.sha256": {"exit": code, "stderr": f"{label} failure"}})
                error = self.refuse()
                self.assertEqual(error.code, "download_failed")
                self.assertEqual(len(self.requested()), sm.MAX_ATTEMPTS)

    def test_rejects_an_oversized_archive(self) -> None:
        big = os.path.join(self.release, "big.tar.gz")
        with open(big, "wb") as handle:
            handle.write(b"\0" * (sm.MAX_ARCHIVE_BYTES + 1))
        archive = support.pack_archive(self.release, VERSION, TARGET)
        self.serve({f"{URL}.sha256": {"file": f"{archive}.sha256"}, URL: {"file": big}})
        self.assertEqual(self.refuse().code, "download_limit")

    def test_rejects_an_oversized_sidecar(self) -> None:
        self.serve({f"{URL}.sha256": {"body": "a" * (sm.MAX_SIDECAR_BYTES + 10)}})
        self.assertEqual(self.refuse().code, "download_limit")


class IntegrityTests(Runner):
    def test_rejects_a_truncated_archive(self) -> None:
        archive = support.pack_archive(self.release, VERSION, TARGET)
        truncated = os.path.join(self.release, "truncated.tar.gz")
        with open(archive, "rb") as source, open(truncated, "wb") as target:
            target.write(source.read()[:-64])
        self.serve({f"{URL}.sha256": {"file": f"{archive}.sha256"}, URL: {"file": truncated}})
        self.assertEqual(self.refuse().code, "digest_mismatch")

    def test_rejects_a_sidecar_that_names_another_file(self) -> None:
        archive = support.pack_archive(self.release, VERSION, TARGET)
        support.write_sidecar(archive, name="memoria-0.5.0-other-target.tar.gz")
        self.serve({f"{URL}.sha256": {"file": f"{archive}.sha256"}, URL: {"file": archive}})
        self.assertEqual(self.refuse().code, "sidecar_mismatch")

    def test_rejects_a_malformed_sidecar(self) -> None:
        archive = support.pack_archive(self.release, VERSION, TARGET)
        self.serve({f"{URL}.sha256": {"body": "not a checksum record\n"}, URL: {"file": archive}})
        self.assertEqual(self.refuse().code, "sidecar_invalid")

    def test_rejects_a_caller_digest_that_disagrees_with_the_sidecar(self) -> None:
        self.serve_release()
        error = self.refuse(INPUT_SHA256="b" * 64)
        self.assertEqual(error.code, "digest_mismatch")
        self.assertEqual(self.requested(), [f"{URL}.sha256"], "the archive is never fetched")

    def test_rejects_a_digest_from_another_architecture(self) -> None:
        other = "aarch64" if TARGET.startswith("x86_64") else "x86_64"
        foreign = support.pack_archive(
            support.scratch("memoria-foreign-"), VERSION, f"{other}-unknown-linux-gnu"
        )
        self.serve_release()
        error = self.refuse(INPUT_SHA256=support.file_digest(foreign))
        self.assertEqual(error.code, "digest_mismatch")


class RuntimeTests(Runner):
    def test_rejects_a_hostile_archive_with_a_valid_checksum(self) -> None:
        self.serve_release(extra=[(f"memoria-{VERSION}-{TARGET}/payload.sh", b"evil\n")])
        self.assertEqual(self.refuse().code, "archive_invalid")
        self.assertEqual(self.path_entries(), [])

    def test_rejects_a_non_elf_executable(self) -> None:
        self.serve_release(binary_bytes=b"#!/bin/sh\necho memoria 0.5.0\n")
        self.assertEqual(self.refuse().code, "binary_invalid")

    def test_rejects_an_elf_for_another_machine(self) -> None:
        with open(support.build_stub(VERSION), "rb") as handle:
            header = bytearray(handle.read())
        other = 0xB7 if sm.ELF_MACHINES[TARGET] == 0x3E else 0x3E
        header[18:20] = other.to_bytes(2, "little")
        self.serve_release(binary_bytes=bytes(header))
        self.assertEqual(self.refuse().code, "binary_invalid")

    def test_rejects_an_executable_that_reports_another_version(self) -> None:
        self.serve_release(binary=support.build_stub(VERSION, "wrong"))
        self.assertEqual(self.refuse().code, "version_mismatch")

    def test_rejects_an_executable_that_fails(self) -> None:
        self.serve_release(binary=support.build_stub(VERSION, "fail"))
        self.assertEqual(self.refuse().code, "binary_unusable")

    def test_rejects_an_executable_that_exceeds_the_runtime_deadline(self) -> None:
        sm.VERSION_DEADLINE_SECONDS = 1
        self.addCleanup(setattr, sm, "VERSION_DEADLINE_SECONDS", 5)
        self.serve_release(binary=support.build_stub(VERSION, "hang"))
        self.assertEqual(self.refuse().code, "binary_unusable")

    def test_rejects_the_expected_version_followed_by_other_output(self) -> None:
        # The whole permitted response is compared. A matching prefix followed
        # by padding and other text is a mismatch, not an accepted answer.
        self.serve_release(binary=support.build_stub(VERSION, "trailing"))
        error = self.refuse()
        self.assertEqual(error.code, "version_mismatch")
        self.assertIn("WRONG TRAILING OUTPUT", error.message)
        self.assertEqual(self.path_entries(), [])
        self.assertEqual(self.outputs(), {})

    def test_rejects_an_executable_that_floods_stdout(self) -> None:
        self.serve_release(binary=support.build_stub(VERSION, "overflow_stdout"))
        error = self.refuse()
        self.assertEqual(error.code, "binary_unusable")
        self.assertIn("stdout", error.message)
        self.assertEqual(self.path_entries(), [])

    def test_rejects_an_executable_that_floods_stderr(self) -> None:
        self.serve_release(binary=support.build_stub(VERSION, "overflow_stderr"))
        error = self.refuse()
        self.assertEqual(error.code, "binary_unusable")
        self.assertIn("stderr", error.message)
        self.assertEqual(self.path_entries(), [])

    def test_an_oversized_extension_header_is_refused_over_the_transport(self) -> None:
        # A valid checksum reaches archive handling, so this fixture exercises
        # the bound rather than the integrity comparison.
        archive = support.pack_pax_bomb(self.release, VERSION, TARGET, 65 * 1024 * 1024)
        self.serve({URL: {"file": archive}, f"{URL}.sha256": {"file": f"{archive}.sha256"}})
        error = self.refuse()
        self.assertIn(error.code, {"archive_limit", "archive_invalid"})
        self.assertEqual(self.path_entries(), [])
        self.assertEqual(self.outputs(), {})
        self.assertFalse(support.cargo_invoked(self.tools))


class VersionCompletionTests(Runner):
    """Both streams must finish inside one deadline before success."""

    def short_deadline(self, seconds: int) -> None:
        self.addCleanup(setattr, sm, "VERSION_DEADLINE_SECONDS", sm.VERSION_DEADLINE_SECONDS)
        sm.VERSION_DEADLINE_SECONDS = seconds

    def test_an_unfinished_stream_is_never_treated_as_an_empty_stream(self) -> None:
        # The executable answers correctly on stdout and exits, but a
        # grandchild holds stderr open past the deadline. The stderr content is
        # unknown, so the answer cannot be accepted. This is the reviewer's
        # scenario, run at the real deadline.
        binary = support.build_stub(VERSION, "held_stderr")
        started = time.monotonic()
        with self.assertRaises(sm.SetupError) as caught:
            sm.check_version(binary, VERSION)
        elapsed = time.monotonic() - started
        self.assertEqual(caught.exception.code, "binary_unusable")
        self.assertIn("stderr", caught.exception.message)
        self.assertIn("unfinished", caught.exception.message)
        # One deadline, plus a small tolerance for scheduling and cleanup.
        self.assertLess(elapsed, sm.VERSION_DEADLINE_SECONDS + 2.0, f"took {elapsed:.2f}s")

    def test_one_deadline_covers_the_child_and_both_readers(self) -> None:
        # The child never exits and a grandchild holds both pipes. A fresh
        # deadline for the wait and for each join would take three deadlines.
        self.short_deadline(2)
        binary = support.build_stub(VERSION, "held_streams_hang")
        started = time.monotonic()
        with self.assertRaises(sm.SetupError) as caught:
            sm.check_version(binary, VERSION)
        elapsed = time.monotonic() - started
        self.assertEqual(caught.exception.code, "binary_unusable")
        self.assertLess(
            elapsed,
            2 * sm.VERSION_DEADLINE_SECONDS + 1.0,
            f"the deadline was multiplied: {elapsed:.2f}s",
        )

    def test_a_missing_reader_result_fails_validation(self) -> None:
        # A reader that records nothing leaves the stream content unknown.
        # That must fail, not default to an accepted empty stream.
        for label in ("stdout", "stderr"):
            with self.subTest(label=label):
                original = sm._read_bounded

                def partial(stream, limit, name, results, process, _label=label):
                    if name == _label:
                        try:
                            stream.close()
                        except OSError:
                            pass
                        return
                    original(stream, limit, name, results, process)

                self.addCleanup(setattr, sm, "_read_bounded", original)
                sm._read_bounded = partial
                binary = support.build_stub(VERSION)
                with self.assertRaises(sm.SetupError) as caught:
                    sm.check_version(binary, VERSION)
                self.assertEqual(caught.exception.code, "binary_unusable")
                self.assertIn(label, caught.exception.message)
                sm._read_bounded = original

    def test_an_unfinished_stream_publishes_no_path_and_no_outputs(self) -> None:
        # The same refusal at the installer boundary.
        self.short_deadline(2)
        self.serve_release(binary=support.build_stub(VERSION, "held_stderr"))
        error = self.refuse()
        self.assertEqual(error.code, "binary_unusable")
        self.assertEqual(self.path_entries(), [])
        self.assertEqual(self.outputs(), {})

    def test_a_well_behaved_executable_still_passes(self) -> None:
        # The completion check must not refuse an ordinary answer.
        binary = support.build_stub(VERSION)
        self.assertEqual(sm.check_version(binary, VERSION), f"memoria {VERSION}")


class PreservationTests(Runner):
    def snapshot(self, directory: str) -> dict:
        found = {}
        for root, _, files in os.walk(directory):
            for name in files:
                path = os.path.join(root, name)
                with open(path, "rb") as handle:
                    found[path] = handle.read()
        return found

    def test_a_successful_install_changes_nothing_in_the_project(self) -> None:
        before = self.snapshot(self.project)
        self.serve_release()
        self.install()
        self.assertEqual(self.snapshot(self.project), before)

    def test_no_path_or_output_is_published_before_validation(self) -> None:
        self.serve_release(binary=support.build_stub(VERSION, "wrong"))
        self.refuse()
        self.assertEqual(self.path_entries(), [])
        self.assertEqual(self.outputs(), {})

    def test_cargo_is_never_invoked(self) -> None:
        self.serve_release()
        self.install()
        self.assertFalse(support.cargo_invoked(self.tools))

    def test_a_failed_install_never_invokes_cargo(self) -> None:
        self.serve({f"{URL}.sha256": {"status": 404}})
        self.refuse()
        self.assertFalse(support.cargo_invoked(self.tools))

    def test_a_command_file_write_failure_is_reported_as_a_failure(self) -> None:
        # Validation completed, so PATH can already hold the entry when the
        # output file rejects the write. The step must still fail. This is a
        # report guarantee, not a rollback guarantee.
        self.serve_release()
        error = self.refuse(GITHUB_OUTPUT="/dev/full")
        self.assertEqual(error.code, "runner_incomplete")
        self.assertIn("GITHUB_OUTPUT", error.message)
        code = sm.main(self.env(GITHUB_OUTPUT="/dev/full"), out=self.out, err=self.err)
        self.assertEqual(code, 1, "a partial publication still fails the step")


class RunnerContractTests(Runner):
    def test_requires_the_workflow_command_files(self) -> None:
        self.serve_release()
        for key in ("GITHUB_PATH", "GITHUB_OUTPUT"):
            with self.subTest(key=key):
                error = self.refuse(**{key: None})
                self.assertEqual(error.code, "runner_incomplete")
                self.assertEqual(self.requested(), [])

    def test_requires_a_writable_runner_temp(self) -> None:
        self.serve_release()
        error = self.refuse(RUNNER_TEMP=os.path.join(self.base, "absent"))
        self.assertEqual(error.code, "runner_incomplete")

    def test_requires_curl(self) -> None:
        empty = os.path.join(self.base, "empty-tools")
        os.makedirs(empty, exist_ok=True)
        error = self.refuse(PATH=empty)
        self.assertEqual(error.code, "prerequisite_missing")

    def test_main_reports_a_refusal_as_an_annotation_and_exit_one(self) -> None:
        self.serve({f"{URL}.sha256": {"status": 404}})
        code = sm.main(self.env(), out=self.out, err=self.err)
        self.assertEqual(code, 1)
        self.assertIn("::error::asset_missing:", self.err.getvalue())

    def test_annotations_escape_control_characters(self) -> None:
        stream = io.StringIO()
        sm.annotate(stream, "error", "a%b\nc\rd")
        self.assertEqual(stream.getvalue().strip(), "::error::a%25b%0Ac%0Dd")


if __name__ == "__main__":
    unittest.main()
