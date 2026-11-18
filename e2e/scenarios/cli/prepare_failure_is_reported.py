#!/usr/bin/env python3
# Covers the fork's review-guide rebuild: provider failure persists an actionable reason and exits nonzero.
import json

from e2e.prepare import fake_claude, review_fixture, stored_state_path

from e2e import Scenario, scenario


@scenario
def main(s: Scenario) -> None:
    repo, _ = review_fixture(s)
    fake_claude(s, response=None)
    result = s.cli_result("prepare", "-w", "--wait", cwd=repo)
    assert result.rc != 0 and "claude" in result.out and "prepare.log" in result.out, result
    path = stored_state_path(s)
    state = json.loads(path.read_text())
    assert state["status"] == "failed" and state["finished_at"], state
    assert "claude" in state["error"], state
    log = (path.parent / "prepare.log").read_text()
    assert "exit status: 23" in log and "last stream event: system" in log, log
    assert "You are an independent reviewer" not in log, "Provider stderr echoed the prompt into the log"
    assert not (path.parent / "guide.json").exists(), "Failed worker installed a guide"
