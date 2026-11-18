#!/usr/bin/env python3
# Covers the fork's review-guide rebuild: opening a PR opts into preparation without stealing focus.
import json

from e2e.prepare import fake_claude, stored_state_path, stored_state_paths

from e2e import Scenario, forge, scenario, wait_cmd


@scenario
def main(s: Scenario) -> None:
    # Gerrit's fixture has pinned source objects locally, so no archive fetch is needed.
    guide = {
        "version": 2,
        "diff": "",
        "summary": "Update a text file.",
        "stories": [
            {
                "title": "Update the example lines",
                "stops": [
                    {
                        "id": "first-line",
                        "claim": "The change updates the first line.",
                        "path": "a.txt",
                        "side": "new",
                        "start_line": 1,
                        "end_line": 1,
                        "untested": True,
                    }
                ],
            }
        ],
        "estimate_minutes": 1,
    }
    fake_claude(
        s,
        response={"guide": guide, "audit": {"stops": [{"id": "first-line", "audit": {"result": "holds"}}]}},
        requires=("Add a.txt", "a.txt", "deliberately switch roles"),
        digest_from_prompt=True,
    )
    s.env["TUICR_PREPARE_ON_OPEN"] = "1"
    tui = forge.gerrit_open(s, "threads")
    tui.wait_for("Add a.txt")

    def preparation_finished() -> bool:
        return any(json.loads(path.read_text())["status"] in ("ready", "failed") for path in stored_state_paths(s))

    wait_cmd(15, preparation_finished)
    state = json.loads(stored_state_path(s).read_text())
    assert state["status"] == "ready" and state["title"] == "Add a.txt", state
    tui.quit()
