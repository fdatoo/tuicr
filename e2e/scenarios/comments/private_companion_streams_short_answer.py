#!/usr/bin/env python3
# Fork review guide: Ask streams a short answer, retains new text, and expands on request.
from e2e.companion import fake_claude

from e2e import Scenario, repos, scenario


@scenario
def main(s: Scenario) -> None:
    repo = repos.git_repo(s.dir / "repo")
    started = s.dir / "start-text"
    release = s.dir / "finish-text"
    announce = s.dir / "announce"
    s.on_exit(release.touch)
    s.on_exit(started.touch)
    s.on_exit(announce.touch)
    fake_claude(
        s,
        f"""
if 'explicitly requested more detail' in prompt:
    assert 'This writes the result. The next step reads it.' in prompt
    answer('The caller builds the result first, then the render loop reads it on every redraw.')
elif "Reviewer's current question:\\na new question" in prompt:
    activity('Read')
    while not Path({str(announce)!r}).exists():
        time.sleep(0.02)
    answer('The retained follow-up is answered. This is the reason. The source confirms it. Hidden extra sentence.')
else:
    assert 'Hidden extra sentence' not in prompt, 'Reveal must not launch another provider'
    assert 'two or three sentences' in prompt and 'one level above' in prompt
    assert '--model=haiku' in sys.argv
    activity('Read')
    while not Path({str(started)!r}).exists():
        time.sleep(0.02)
    delta('This writes the result. ')
    while not Path({str(release)!r}).exists():
        time.sleep(0.02)
    delta('The next step reads it.')
    finish('This writes the result. The next step reads it.')
""",
    )
    tui = s.start(cwd=repo)
    tui.wait_for("Unstaged changes").keys("Enter").cursor_on("unstaged line")
    tui.keys("a").wait_for(r"Private companion .*README\.md:")
    tui.type("What does this change do?").keys("Enter").wait_for("Reading source")
    started.touch()
    tui.wait_for("This writes the result").wait_for("Streaming")
    assert not list(s.dir.glob("**/companion/*/*.json")), "a partial answer reached disk"
    tui.type("a new question").keys("Enter").wait_for(r"still working on README\.md:")
    tui.wait_for("Question: a new question")
    release.touch()
    tui.wait_for("The next step reads it").wait_gone("Streaming")
    tui.wait_for("Question: a new question").keys("C-o").wait_for("The caller builds the result first")
    tui.wait_gone("Streaming")
    tui.wait_for("Question: a new question").keys("Enter").wait_for("Reading source")
    tui.keys("Escape").cmd("1")
    announce.touch()
    tui.wait_for(r"Private answer ready at README\.md:5")
    tui.cmd("ask").wait_for("The retained follow-up is answered.").wait_for("Ctrl-o show full answer")
    assert "Hidden extra sentence" not in tui.screen()
    assert any("Hidden extra sentence" in record.read_text() for record in s.dir.glob("**/companion/*/*.json"))
    tui.keys("C-o").wait_for("Hidden extra sentence").wait_gone("Ctrl-o show full answer")
    tui.keys("Escape").wait_for("◆unstaged line").quit()
