#!/usr/bin/env python3
"""Independently validate one generated consumer workflow file.

The Memoria CLI renders the consumer workflow from one embedded template.
This checker parses the rendered bytes with a general YAML parser, so a
template defect cannot hide behind the code that produced it.

PyYAML follows YAML 1.1, which reads the bare key `on` as the boolean `True`.
The checker accepts both spellings of that key and also inspects the raw
bytes, so the file stays readable for a person and for GitHub Actions.

The checker is a test-only tool. Production code needs no YAML library.
"""

from __future__ import annotations

import sys

import yaml

#: Runner labels the setup Action supports.
PERMITTED_RUNNERS = {"ubuntu-24.04", "ubuntu-latest", "ubuntu-24.04-arm"}

SETUP_ACTION = "viktordanov/rs-memoria@"


class WorkflowProblem(Exception):
    """One consumer workflow requirement that the file does not meet."""


def events(document: dict) -> object:
    """Return the workflow trigger section under either spelling of `on`."""
    if "on" in document:
        return document["on"]
    if True in document:
        return document[True]
    raise WorkflowProblem("the workflow has no trigger section")


def check(text: str) -> dict:
    """Examine one rendered workflow and return its parsed document."""
    if "\r" in text:
        raise WorkflowProblem("the workflow holds a carriage return; the template writes LF only")
    if not text.endswith("\n"):
        raise WorkflowProblem("the workflow does not end with a line feed")
    document = yaml.safe_load(text)
    if not isinstance(document, dict):
        raise WorkflowProblem("the workflow is not a YAML mapping")

    if "\non:" not in text and not text.startswith("on:"):
        raise WorkflowProblem("the workflow has no top-level `on:` key in its bytes")
    triggers = events(document)
    names = set(triggers) if isinstance(triggers, (list, dict)) else {triggers}
    for required in ("push", "pull_request"):
        if required not in names:
            raise WorkflowProblem(f"the workflow does not run on {required}")
    if "pull_request_target" in names:
        raise WorkflowProblem("the workflow must not use pull_request_target")

    if document.get("permissions") != {"contents": "read"}:
        raise WorkflowProblem("the workflow must request contents: read only")

    jobs = document.get("jobs")
    if not isinstance(jobs, dict) or len(jobs) != 1:
        raise WorkflowProblem("the workflow must hold exactly one job")
    job = next(iter(jobs.values()))
    if job.get("runs-on") not in PERMITTED_RUNNERS:
        raise WorkflowProblem(f"runs-on {job.get('runs-on')!r} is not a supported runner label")
    if not isinstance(job.get("timeout-minutes"), int):
        raise WorkflowProblem("the job must declare a timeout")

    steps = job.get("steps")
    if not isinstance(steps, list) or not steps:
        raise WorkflowProblem("the job has no steps")
    uses = [step.get("uses", "") for step in steps]
    checkout = [step for step in steps if str(step.get("uses", "")).startswith("actions/checkout@")]
    if len(checkout) != 1:
        raise WorkflowProblem("the job must check out the repository exactly once")
    if checkout[0].get("with", {}).get("persist-credentials") is not False:
        raise WorkflowProblem("the checkout step must set persist-credentials: false")
    setup = [step for step in steps if str(step.get("uses", "")).startswith(SETUP_ACTION)]
    if len(setup) != 1:
        raise WorkflowProblem(f"the job must use {SETUP_ACTION}<ref> exactly once")
    if "version" not in setup[0].get("with", {}):
        raise WorkflowProblem("the setup step must pin an explicit version input")
    runs = [str(step.get("run", "")).strip() for step in steps if "run" in step]
    for required in ("memoria --version", "memoria check"):
        if required not in runs:
            raise WorkflowProblem(f"the job must invoke `{required}`")
    for action in uses:
        if action and "@" not in action:
            raise WorkflowProblem(f"the step `uses: {action}` has no ref")
    return document


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print("usage: check_consumer_workflow.py FILE", file=sys.stderr)
        return 2
    with open(argv[1], "r", encoding="utf-8") as handle:
        text = handle.read()
    try:
        check(text)
    except WorkflowProblem as problem:
        print(f"generated workflow is invalid: {problem}", file=sys.stderr)
        return 1
    print(f"generated workflow {argv[1]} meets the consumer contract")
    return 0


if __name__ == "__main__":  # pragma: no cover - process entry point
    raise SystemExit(main(sys.argv))
