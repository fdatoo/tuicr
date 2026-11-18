#!/usr/bin/env python3
# Fork review guide: reload cancels immediately, frees Ask and labels answers written against old code.
from e2e.companion import fake_claude

from e2e import Scenario, repos, scenario, wait_file


@scenario
def main(s: Scenario) -> None:
    repo = repos.git_repo(s.dir / "repo")
    started = s.dir / "asked"
    release = s.dir / "release"
    s.on_exit(release.touch)
    fake_claude(
        s,
        f"""
if 'first request fails' in prompt:
    delta('cut off by an error')
    sys.exit(12)
elif 'outstanding question' in prompt:
    activity('Read')
    Path({str(started)!r}).write_text('ready')
    while not Path({str(release)!r}).exists():
        time.sleep(0.02)
    answer('stale answer must not appear')
elif 'old code question' in prompt:
    answer('Explanation before reload.')
else:
    assert 'new code question' in prompt
    assert 'replacement after reload' in prompt
    assert 'Explanation before reload' not in prompt
    answer('New answer after reload.')
""",
    )
    tui = s.start(cwd=repo)
    tui.wait_for("Unstaged changes").keys("Enter").cursor_on("unstaged line")
    tui.keys("a").type("first request fails").keys("Enter").wait_for("Ask failed")
    tui.wait_for("Question: first request fails").wait_gone("cut off by an error")
    tui.keys("C-u").type("old code question").keys("Enter").wait_for("Explanation before reload")
    tui.wait_gone("Streaming")
    tui.type("outstanding question").keys("Enter").wait_for("Reading source")
    wait_file(started)

    tui.keys("Escape").wait_gone("Private companion")
    readme = repo / "README.md"
    readme.write_text(readme.read_text().replace("unstaged line", "replacement after reload"))
    tui.cmd("e").wait_for("replacement after reload").cursor_on("replacement after reload")
    tui.keys("a").wait_for("earlier version").wait_for("Explanation before reload")
    tui.wait_for("Question: outstanding question")
    tui.keys("C-u").type("new code question").keys("Enter").wait_for("New answer after reload")
    tui.reject("stale answer must not appear")
    tui.keys("Escape").quit()
