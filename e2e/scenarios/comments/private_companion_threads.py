#!/usr/bin/env python3
# Fork review guide: threads preserve range, navigator anchor and privacy; only saved feedback exports.
from e2e.companion import fake_claude

from e2e import Scenario, repos, scenario, wait_file


@scenario
def main(s: Scenario) -> None:
    repo = repos.git_repo(s.dir / "repo")
    fake_claude(
        s,
        """
if 'Draft one concise' in prompt:
    assert 'ask them to add a test for the timeout' in prompt
    assert 'first private answer' in prompt and 'followup private answer' in prompt
    assert 'Never state a suspicion' in prompt
    answer('Could you add a regression test for the timeout?')
elif "Reviewer's current question:\\nprivate followup" in prompt:
    assert 'first private answer' in prompt
    answer('followup private answer')
elif "Reviewer's current question:\\nprivate reply" in prompt:
    assert 'Existing review comment' in prompt and 'accepted removal feedback' in prompt
    assert 'Related private exchange at gone.txt:' in prompt
    answer('private reply answer')
elif "Reviewer's current question:\\nprivate new-side question" in prompt:
    assert 'README.md:4 (new)' in prompt
    answer('new-side context answer')
else:
    assert "Reviewer's current question:\\nprivate range\\nquestion" in prompt
    assert 'gone.txt:1–2 (old)' in prompt and 'Selected code' in prompt
    assert 'delete me\\nline' in prompt
    answer('first private answer')
""",
    )
    tui = s.start("-r", "HEAD~2..HEAD", cwd=repo).wait_for("Files · 0/8").keys("Escape")
    second = s.start(
        "-r",
        "HEAD~2..HEAD",
        cwd=repo,
        name=f"{s.name}-second",
        env={"HOME": str(s.dir / "home"), "XDG_DATA_HOME": str(s.dir / "data")},
    ).wait_for("Files · 0/8")
    second.cmd("ask history").wait_for("No private exchanges for this review yet").keys("Escape")

    tui.cursor_on("4 ▌ second line").keys("a").wait_for("Private companion")
    tui.type("private new-side question").keys("Enter").wait_for("new-side context answer")
    tui.keys("Escape").wait_gone("Private companion")
    tui.cursor_on("1 ▌ delete me").keys("v", "j", ";a").wait_for("Private companion")
    tui.paste("private range\nquestion").keys("Enter").wait_for("first private answer")
    tui.wait_gone("Streaming")
    tui.keys("Escape", "a").wait_for(r"Private companion .*gone.txt:1–2 \(old\)")
    tui.wait_for("first private answer").type("private followup").keys("Enter").wait_for("followup private answer")
    tui.wait_gone("Streaming")
    tui.keys("C-d").wait_for("Nothing to send")
    tui.type("ask them to add a test for the timeout").keys("C-d").wait_for("Could you add a regression test")
    tui.wait_for("Feedback draft")

    exported = s.dir / "review.md"
    tui.keys("Escape").cmd(f"file {exported}").wait_for("No comments to write")
    assert not exported.exists(), "a private answer or proposal was exported"

    tui.cmd("o1").keys("v", "j", "a").wait_for("Could you add a regression test")
    tui.keys("C-e").wait_for("COMMENT").wait_for("Could you add a regression test")
    tui.keys("C-u").type("accepted removal feedback").keys("C-s").wait_for("NORMAL")
    comment = s.session()["files"]["gone.txt"]["line_comments"]["2"][0]
    assert comment["content"] == "accepted removal feedback", comment
    assert comment["side"] == "old" and comment["line_range"] == {"start": 1, "end": 2}, comment

    tui.keys(";h", ";j", "a").wait_for("Private companion").wait_for("accepted removal feedback")
    tui.type("private reply").keys("Enter").wait_for("private reply answer")
    tui.wait_gone("Streaming")
    tui.keys("Tab").wait_for("Private review history").wait_for("private range").wait_for("private reply")
    tui.cmd(f"file {exported}")
    report = wait_file(exported).read_text()
    assert "accepted removal feedback" in report
    assert "private answer" not in report and "private reply" not in report and "regression test" not in report
    assert "new-side context answer" not in report

    second.cmd("ask history").wait_for("private reply").keys("Enter").wait_for("private reply answer")
    assert "line" in second.cursor_row() and "gone.txt" in second.screen()
    second.keys("Escape").wait_gone("Private companion").keys(":q!", "Enter").wait_exit()
    tui.quit()
