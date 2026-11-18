#!/usr/bin/env python3
# Review-guide rebuild: walk five supporting hunks, resume the same sample, and stop after five verdicts.
import json
import re

from e2e import Scenario, repos, scenario


@scenario
def main(s: Scenario) -> None:
    repo = repos.empty_repo(s.dir / "repo")
    repos.write(repo / "README.md", "# Spot-check fixture\n")
    repos.git(repo, "add", "README.md")
    repos.git(repo, "commit", "-qm", "base")
    repos.write(repo / "main.txt", "main change\n")
    hunks = []
    for index in range(7):
        name = f"support-{index}.txt"
        repos.write(repo / name, f"support change {index}\n")
        hunks.append({"path": name, "old_start": 0, "new_start": 1})
    repos.git(repo, "add", "-A")
    digest = s.cli("guide", "outline", "-w", cwd=repo).splitlines()[0].removeprefix("diff: ")
    guide = {
        "version": 2,
        "diff": digest,
        "stories": [
            {
                "title": "Main change",
                "stops": [
                    {"id": "main", "claim": "Main change works", "path": "main.txt", "start_line": 1, "end_line": 1}
                ],
            }
        ],
        "supporting": [{"label": "Wiring", "basis": "claimed", "hunks": hunks}],
    }
    supplied = repos.write(s.dir / "guide.json", json.dumps(guide))
    tui = s.start("-w", "--guide", supplied, cwd=repo).wait_for(r"Review (?:guide )?ready")
    spot_cursor = r"(?m)▶.*support change (\d)"
    tui.cmd("spot").wait_for("spot-check 1 of 5").wait_for(spot_cursor)
    first = re.search(spot_cursor, tui.screen())
    assert first is not None, tui.screen()
    tui.keys("Escape").wait_gone("spot-check 1 of 5").cmd("spot").wait_for("spot-check 1 of 5")
    tui.wait_for(rf"▶.*support change {first[1]}")
    tui.keys(".").wait_for("spot-check 2 of 5").wait_for(spot_cursor)
    second = re.search(spot_cursor, tui.screen())
    assert second is not None and second[1] != first[1], tui.screen()
    tui.quit()
    tui = s.start("-w", "--guide", supplied, cwd=repo, keep=True).wait_for(r"Review (?:guide )?ready")
    tui.cmd("spot").wait_for("spot-check 2 of 5").wait_for(rf"▶.*support change {second[1]}")
    tui.keys("x").wait_for("COMMENT").keys("Escape").wait_for("spot-check 2 of 5")
    tui.keys("x").wait_for("COMMENT").type("This supporting hunk needs a fix").keys("Enter")
    tui.wait_for("spot-check 3 of 5").keys("p").wait_for("spot-check 4 of 5")
    tui.keys(".").wait_for("spot-check 5 of 5").keys(".")
    result = "Spot-check complete: 3 checked out, 1 wrong, 1 parked"
    tui.wait_for(result).wait_gone("spot-check 5 of 5")
    tui.cmd("spot").wait_for(result).wait_gone("spot-check 1 of 5")
    tui.cmd("coverage").wait_for("5 recorded").wait_for("7 hunks: 3 verified, 0 dismissed, 4 unverified")
    tui.keys("Escape").wait_gone("5 recorded")
    assert s.session_has("This supporting hunk needs a fix"), s.session_json()
    tui.quit()
