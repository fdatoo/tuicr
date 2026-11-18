#!/usr/bin/env python3
# Fork review guide: guide explanations and followups need no call; a new question sends story, stop and rules.
import json
import re

from e2e.companion import fake_claude
from e2e.prepare import review_fixture

from e2e import Scenario, repos, scenario


def dock_screen(screen: str) -> str:
    lines = screen.splitlines()
    row = next(i for i, line in enumerate(lines) if "Private companion" in line)
    col = lines[row].index("Private companion") - 1
    return "\n".join(line[col:].rstrip("│┃") for line in lines[row:])


@scenario
def main(s: Scenario) -> None:
    repo, guide = review_fixture(s)
    explanation = str(guide["stories"][0]["stops"][0]["explanation"])
    supplied = repos.write(s.dir / "guide.json", json.dumps(guide))
    repos.write(s.dir / "config/tuicr/reviewer.md", "Check whether a literal is allocated.\n")
    called = s.dir / "provider-called"
    fake_claude(
        s,
        f"""
Path({str(called)!r}).touch()
assert 'Introduce a greeting' in prompt
assert 'The greeting is a static string.' in prompt
assert {explanation!r} in prompt
assert 'Check whether a literal is allocated.' in prompt
assert 'It is a literal in the source.' in prompt
assert "Reviewer's current question:\\nWhat does its caller see?" in prompt
answer('The caller sees a shared immutable string.')
""",
    )
    tui = s.start("-w", "--guide", supplied, cwd=repo, keep=True)
    tui.wait_for("Review ready:").cmd("guide").wait_for("The greeting is a static string")
    tui.keys("a").wait_for("Prepared explanation").wait_for("Prepared questions")
    tui.wait_for("› Where is the string created").wait_for("· Does every caller get the same greeting")
    dock = dock_screen(tui.screen(join=False))
    assert dock.index("Prepared explanation") < dock.index(explanation[:35]) < dock.index("Prepared questions"), dock
    assert "Prepared answer" not in dock and "claude" not in dock, dock
    tui.keys("Enter").wait_for("It is a literal in the source")
    dock = dock_screen(tui.screen(join=False))
    assert re.search(r"› Where is the string created.*\n\s+Prepared answer.*\n\s+It is a literal", dock), dock
    assert not called.exists(), "viewing a prepared follow-up called the provider"
    tui.keys("Escape").wait_gone("Private companion")
    tui.keys("a").wait_for("› Where is the string created")
    tui.type("What does its caller see?").keys("Enter").wait_for("The caller sees a shared immutable string")
    dock = dock_screen(tui.screen(join=False))
    assert re.search(r"\n\s*You\s*\n.*\n\s*claude\s*\n\s*The caller sees", dock), dock
    tui.keys("Escape").wait_gone("Private companion")
    tui.resize(60, 15).keys("a").wait_for("Private companion").wait_for("Question:")
    screen = tui.screen(join=False)
    code = screen.split("Private companion", 1)[0]
    dock = dock_screen(screen)
    assert re.search(r"Prepared explanation.*\n A caller asks.*…", dock), dock
    assert "Prepared questions" in dock and "› Where is the string created" in dock and "Question:" in dock, dock
    assert re.search(r"1 .*fn greeting\(\)", code), screen
    assert re.search(r'2 .*"hello"', code), screen
    assert re.search(r"3 .*\}", code), screen
    tui.keys("Escape").wait_gone("Private companion").keys(":q!", "Enter").wait_exit()
