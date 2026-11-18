#!/usr/bin/env python3
# Covers the fork's review-guide rebuild: guide check reports all unresolved paths and line ranges.
import json

from e2e.prepare import review_fixture

from e2e import Scenario, repos, scenario


@scenario
def main(s: Scenario) -> None:
    repo, guide = review_fixture(s)
    stops = guide["stories"][0]["stops"]
    stops[0].update(start_line=94, end_line=95)
    stops.append({**stops[0], "id": "missing", "path": "missing.rs", "start_line": 1, "end_line": 2})
    supplied = repos.write(s.dir / "bad-anchors.json", json.dumps(guide))
    result = s.cli_result("guide", "check", "-w", supplied, cwd=repo)
    assert result.rc != 0, result
    assert "bad-anchors.json" in result.out, result
    assert "reviewed.rs (new side, lines 94-95)" in result.out, result
    assert "missing.rs (new side, lines 1-2)" in result.out, result
