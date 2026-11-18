#!/usr/bin/env python3
# Covers the fork's review-guide rebuild: a supplied guide is audited without a write pass.
import json

from e2e.prepare import fake_claude, review_fixture, stored_state_path

from e2e import Scenario, repos, scenario


@scenario
def main(s: Scenario) -> None:
    repo, guide = review_fixture(s)
    guide.pop("estimate_minutes")
    guide["provenance"] = {"author": "reviewer", "audited": True, "provider": "fake-author-provider"}
    guide["stories"][0]["stops"][0]["audit"] = {"result": "doubtful", "note": "fake-author-verdict"}
    supplied = repos.write(s.dir / "supplied.json", json.dumps(guide))
    audit = {
        "stops": [
            {
                "id": "greeting",
                "audit": {"result": "doubtful", "note": "Inspect the caller."},
                "explanation": "The audit could not confirm which caller uses this.",
                "explanation_edits": [
                    {
                        "sentence": "This function returns a literal rather than allocating.",
                        "replacement": "This function returns a string literal.",
                    }
                ],
                "untested": True,
            }
        ],
        "missed": [],
    }
    calls = fake_claude(
        s,
        response=audit,
        requires=("Try to refute", "The greeting is a static string.", '"audited":false', '"result":"unchecked"'),
        forbids=("Write a review guide for a person", "fake-author-provider", "fake-author-verdict"),
    )
    result = s.cli_result("prepare", "-w", "--guide", supplied, "--wait", cwd=repo)
    assert result.rc == 0, result
    path = stored_state_path(s)
    stored = json.loads((path.parent / "guide.json").read_text())
    assert stored["estimate_minutes"] == 2, stored
    assert stored["provenance"]["author"] == "supplied" and stored["provenance"]["audited"], stored
    stop = stored["stories"][0]["stops"][0]
    assert stop["claim"] == "The greeting is a static string.", stop
    assert stop["explanation"] == "A caller asks for a greeting. This function returns a string literal.", stop
    assert stop["audit"] == {"result": "doubtful", "note": "Inspect the caller."}, stop
    assert calls.read_text().splitlines() == ["called"], "Only the audit pass should run"
