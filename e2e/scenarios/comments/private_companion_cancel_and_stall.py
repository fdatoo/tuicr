#!/usr/bin/env python3
# Ask cancellation frees the slot, and one stalled request is not retried.
from e2e.companion import fake_claude

from e2e import Scenario, repos, scenario


@scenario
def main(s: Scenario) -> None:
    repo = repos.git_repo(s.dir / "repo")
    calls = s.dir / "attempts"
    fake_claude(
        s,
        f"""
question = prompt.split("Reviewer's current question:\\n", 1)[1].splitlines()[0]
with Path({str(calls)!r}).open('a') as log:
    log.write(question + '\\n')
if question == 'replacement question':
    answer('The replacement completed.')
else:
    activity('Read')
    while True:
        # Transport noise is not progress and must not keep Ask alive.
        emit({{'type': 'keepalive'}})
        time.sleep(0.25)
""",
    )
    tui = s.start(cwd=repo)
    tui.wait_for("Unstaged changes").keys("Enter").cursor_on("unstaged line")
    tui.keys("a").type("first question").keys("Enter")
    tui.wait_for("Ctrl-x cancel.*Reading source")
    tui.keys("Escape").wait_for("NORMAL").cmd("1").keys("a")
    tui.type("replacement question").keys("Enter")
    tui.wait_for(r"Ask is still working on README\.md:5").wait_for("Question: replacement question")
    tui.keys("C-x").wait_for("Cancelled Ask at README.md:5").wait_gone("Ask is still working")
    tui.wait_for("Question: replacement question").keys("Enter").wait_for("The replacement completed.")

    tui.type("stalled question").keys("Enter").wait_for("Reading source")
    tui.wait_for("provider stalled without progress", timeout=55)
    tui.wait_for("Enter to retry").wait_for("Question: stalled question")
    assert calls.read_text().splitlines() == ["first question", "replacement question", "stalled question"]
    assert len(list(s.dir.glob("**/companion/*/*.json"))) == 1
    tui.keys("Escape").wait_for("NORMAL").quit()
