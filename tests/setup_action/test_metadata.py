"""Action metadata and consumer-workflow parsing, done independently.

These fixtures parse `action.yml` with a general YAML parser instead of
trusting the code that reads it. PyYAML is a test-only dependency; the
production installer needs no YAML library.
"""

from __future__ import annotations

import unittest

import yaml

import support
from check_consumer_workflow import WorkflowProblem, check

GOOD_WORKFLOW = """\
name: Memoria documentation
on: [push, pull_request]
permissions:
  contents: read
jobs:
  memoria:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    steps:
      - uses: actions/checkout@1111111111111111111111111111111111111111
        with:
          persist-credentials: false
      - name: Set up Memoria
        id: memoria
        uses: viktordanov/rs-memoria@v0.5.0
        with:
          version: '0.5.0'
      - run: memoria --version
      - run: memoria check
"""


class ActionMetadataTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        with open(support.ACTION, "r", encoding="utf-8") as handle:
            cls.text = handle.read()
        cls.document = yaml.safe_load(cls.text)

    def test_is_a_composite_action_with_one_bash_step(self) -> None:
        runs = self.document["runs"]
        self.assertEqual(runs["using"], "composite")
        self.assertEqual(len(runs["steps"]), 1)
        self.assertEqual(runs["steps"][0]["shell"], "bash")

    def test_names_the_action_setup_memoria(self) -> None:
        self.assertEqual(self.document["name"], "setup-memoria")
        self.assertIn("description", self.document)

    def test_declares_both_documented_inputs(self) -> None:
        inputs = self.document["inputs"]
        self.assertEqual(set(inputs), {"version", "sha256"})
        self.assertEqual(inputs["version"]["default"], "0.5.0")
        self.assertEqual(inputs["sha256"]["default"], "")
        self.assertFalse(inputs["version"]["required"])

    def test_forwards_inputs_through_the_environment_only(self) -> None:
        step = self.document["runs"]["steps"][0]
        self.assertEqual(
            step["env"],
            {"INPUT_VERSION": "${{ inputs.version }}", "INPUT_SHA256": "${{ inputs.sha256 }}"},
        )
        # No caller-controlled value is interpolated into the shell source.
        self.assertNotIn("inputs.", step["run"])

    def test_quotes_the_action_path_and_runs_the_installer(self) -> None:
        run = self.document["runs"]["steps"][0]["run"]
        self.assertIn('python3 "${GITHUB_ACTION_PATH}/scripts/setup-memoria.py"', run)
        self.assertIn("set -euo pipefail", run)

    def test_maps_both_outputs_from_the_install_step(self) -> None:
        outputs = self.document["outputs"]
        self.assertEqual(set(outputs), {"version", "path"})
        self.assertEqual(outputs["version"]["value"], "${{ steps.install.outputs.version }}")
        self.assertEqual(outputs["path"]["value"], "${{ steps.install.outputs.path }}")
        self.assertEqual(self.document["runs"]["steps"][0]["id"], "install")

    def test_declares_no_cargo_or_rust_input(self) -> None:
        self.assertNotIn("cargo", self.text.lower())
        self.assertNotIn("rustup", self.text.lower())


class ConsumerWorkflowCheckerTests(unittest.TestCase):
    def test_accepts_the_documented_consumer_workflow(self) -> None:
        document = check(GOOD_WORKFLOW)
        # YAML 1.1 reads the bare `on` key as a boolean; the checker accepts
        # both spellings and still requires the literal bytes.
        self.assertTrue("on" in document or True in document)

    def refuse(self, text: str) -> str:
        with self.assertRaises(WorkflowProblem) as caught:
            check(text)
        return str(caught.exception)

    def test_requires_the_literal_on_key_in_the_bytes(self) -> None:
        text = GOOD_WORKFLOW.replace("on: [push, pull_request]\n", "")
        self.assertIn("`on:`", self.refuse(text))

    def test_rejects_pull_request_target(self) -> None:
        text = GOOD_WORKFLOW.replace("[push, pull_request]", "[push, pull_request_target]")
        self.assertIn("pull_request", self.refuse(text))

    def test_rejects_write_permissions(self) -> None:
        text = GOOD_WORKFLOW.replace("contents: read", "contents: write")
        self.assertIn("contents: read", self.refuse(text))

    def test_rejects_an_unsupported_runner(self) -> None:
        text = GOOD_WORKFLOW.replace("ubuntu-24.04", "ubuntu-22.04")
        self.assertIn("runs-on", self.refuse(text))

    def test_accepts_every_supported_runner(self) -> None:
        for runner in ("ubuntu-24.04", "ubuntu-latest", "ubuntu-24.04-arm"):
            with self.subTest(runner=runner):
                check(GOOD_WORKFLOW.replace("runs-on: ubuntu-24.04", f"runs-on: {runner}"))

    def test_rejects_a_checkout_that_keeps_credentials(self) -> None:
        text = GOOD_WORKFLOW.replace("persist-credentials: false", "persist-credentials: true")
        self.assertIn("persist-credentials", self.refuse(text))

    def test_rejects_an_unpinned_action(self) -> None:
        text = GOOD_WORKFLOW.replace(
            "      - run: memoria --version\n", "      - uses: actions/cache\n      - run: memoria --version\n"
        )
        self.assertIn("has no ref", self.refuse(text))

    def test_rejects_a_missing_check_step(self) -> None:
        self.assertIn("memoria check", self.refuse(GOOD_WORKFLOW.replace("      - run: memoria check\n", "")))

    def test_rejects_carriage_returns(self) -> None:
        self.assertIn("carriage return", self.refuse(GOOD_WORKFLOW.replace("\n", "\r\n")))


if __name__ == "__main__":
    unittest.main()
