#!/usr/bin/env python3
# Review-guide rebuild: the scoped approval posts saved drafts and an editable, factual note; no private answers.
import json

from e2e.coverage import PRIVATE_ANSWER, guided_pr

from e2e import Scenario, scenario


@scenario
def main(s: Scenario) -> None:
    tui = guided_pr(s)
    tui.keys("r").wait_for("═══ ✓ PR #1")
    tui.cmd("guide").wait_for("The README describes the new example")
    tui.keys("a").wait_for("Prepared questions").keys("Enter").wait_for(PRIVATE_ANSWER)
    tui.keys("Escape").wait_gone("Private companion").keys(".").wait_for("story 2 of 2")
    tui.cmd("submit").wait_for("> Approve what I verified").wait_for("Stops verified: 1/2")
    tui.keys("k", "Enter").wait_gone("Save and come back").wait_for("Saved to")
    assert tui.posts() == [], "Save and come back sent a review"
    assert s.session()["pr_description_reviewed"] is True, "Save and come back did not persist the review mark"
    tui.cmd("submit").wait_for("> Approve what I verified").keys("k", "k", "Enter")
    tui.wait_for("Could you split this change").wait_for("Document the example").wait_for("Print a greeting")
    assert tui.posts() == [], "opening the split draft sent a review"
    tui.keys("Enter").wait_for("Review comment added")
    tui.keys("'").wait_for(r"▶.*pub fn greet").comment("Saved inline feedback")
    tui.cmd("submit").wait_for("> Approve what I verified").keys("Enter")
    tui.wait_for("Coverage note").wait_for("verified 1 of 2 stops").wait_for(r"\[e\] edit note")
    assert tui.posts() == [], "the preview posted before approval"
    tui.keys("e").wait_for("COVERAGE NOTE").type(" Reviewed in one sitting.").keys("Enter")
    tui.wait_for("Reviewed in one sitting").keys("y").wait_for("Submitted GitHub review #1001")

    [post] = tui.posts()
    payload = post["payload"]
    assert payload["event"] == "APPROVE", payload
    body = payload["body"]
    assert "Could you split this change" in body, body
    assert "- Document the example\n- Print a greeting" in body, body
    assert "Reviewed with a guide: verified 1 of 2 stops (story 1)" in body, body
    assert body.endswith("Not verified: story 2. Reviewed in one sitting."), body
    assert [item["body"] for item in payload["comments"]] == ["Saved inline feedback"], payload
    assert PRIVATE_ANSWER not in json.dumps(payload), payload
    tui.quit()
