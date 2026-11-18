#!/usr/bin/env python3
# Covers the fork's review-guide rebuild: waiting prepares and audits a tiny diff.
import hashlib
import json
import stat
import subprocess

from e2e.core import child_env
from e2e.prepare import fake_claude, review_fixture, stored_state_path

from e2e import TUICR_BIN, Scenario, scenario, wait_file, wait_grep


@scenario
def main(s: Scenario) -> None:
    repo, guide = review_fixture(s)
    audit = {"stops": [{"id": "greeting", "audit": {"result": "holds"}, "untested": True}], "missed": []}
    release = s.dir / "release-claude"
    calls = fake_claude(
        s,
        response={"guide": guide, "audit": audit},
        requires=(guide["diff"], "reviewed.rs", "deliberately switch roles", "Do not run code"),
        release=release,
    )
    result = s.cli_result("prepare", "-w", cwd=repo)
    assert result.rc == 0 and "Watch with: tail -f" in result.out, result

    wait_log = s.dir / "wait.log"
    args = [TUICR_BIN, "prepare", "-w", "--wait"]
    env = child_env(s.path_of(s.name), s.env["PATH"], s.env)
    try:
        wait_file(calls)
        with (
            wait_log.open("w") as output,
            subprocess.Popen(
                args, cwd=repo, env=env, stdout=output, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL
            ) as waiter,
        ):
            try:
                wait_grep(wait_log, "already preparing; waiting")
                assert waiter.poll() is None, "prepare --wait returned before the provider finished"
                release.touch()
                assert waiter.wait(timeout=10) == 0, wait_log.read_text()
            finally:
                if waiter.poll() is None:
                    waiter.kill()
    finally:
        release.touch()
    assert "Review guide ready" in wait_log.read_text(), wait_log.read_text()

    cached = s.cli_result("prepare", "-w", "--wait", cwd=repo)
    assert cached.rc == 0 and "already ready" in cached.out, cached
    path = stored_state_path(s)
    state = json.loads(path.read_text())
    stored = json.loads((path.parent / "guide.json").read_text())
    assert state["status"] == "ready" and state["diff"] == guide["diff"], state
    assert state["stops"] == 1 and state["estimate_minutes"] == 2, state
    assert stored["provenance"]["author"] == "reviewer" and stored["provenance"]["audited"], stored
    assert stored["stories"][0]["stops"][0]["audit"]["result"] == "holds", stored
    assert calls.read_text().splitlines() == ["called"], "A tiny diff should use one combined call"
    assert stat.S_IMODE((path.parent / "prepare.log").stat().st_mode) == 0o600, "Log is not private"

    assert stored["provenance"]["rules_digest"] == "no-reviewer-rules"
    rules = s.path_of(s.name) / "config/tuicr/reviewer.md"
    rules.parent.mkdir(parents=True, exist_ok=True)
    rules_text = "Check that the return value does not allocate.\n"
    rules.write_text(rules_text)
    fake_claude(s, response={"guide": guide, "audit": audit}, requires=(rules_text, "Reviewer rules"))
    refreshed = s.cli_result("prepare", "-w", "--wait", cwd=repo)
    assert refreshed.rc == 0 and "Review guide ready for" in refreshed.out, refreshed
    assert calls.read_text().splitlines() == ["called", "called"], "Changing reviewer.md must re-run preparation"
    stored = json.loads((path.parent / "guide.json").read_text())
    assert stored["provenance"]["rules_digest"] == hashlib.sha256(rules_text.encode()).hexdigest(), stored
