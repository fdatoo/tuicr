#!/usr/bin/env python3
# Fork review guide: invalid provider responses are retried once and never persisted.
from e2e.companion import fake_claude

from e2e import Scenario, repos, scenario


@scenario
def main(s: Scenario) -> None:
    repo = repos.git_repo(s.dir / "repo")
    calls = s.dir / "attempts"
    release = s.dir / "release-garbage"
    s.on_exit(release.touch)
    fake_claude(
        s,
        f"""
with Path({str(calls)!r}).open('a') as calls:
    calls.write('call\\n')
assert 'What does this do?' in prompt
delta('A real sentence. ')
delta('[tool ca')
delta('ll: Read]\\n')
delta('```json\\n')
delta('{{"lookups":[{{"path":"secret-protocol.txt"}}]}}\\n```')
activity('Grep')
while not Path({str(release)!r}).exists():
    time.sleep(0.02)
finish('A real sentence. [tool call: Read]')
""",
    )
    tui = s.start(cwd=repo)
    tui.wait_for("Unstaged changes").keys("Enter").cursor_on("unstaged line")
    tui.keys("a").type("What does this do?").keys("Enter")
    tui.wait_for("Searching source").wait_for("A real sentence")
    screen = tui.screen()
    assert "[tool" not in screen and "secret-protocol.txt" not in screen, screen
    release.touch()
    tui.wait_for("No answer saved").wait_for(r"Question: What does this do\?")
    assert calls.read_text().splitlines() == ["call", "call"]
    assert not list(s.dir.glob("**/companion/*/*.json"))
    tui.keys("Escape").cmd("ask history").wait_for("No private exchanges for this review yet")
    tui.keys("Escape").quit()
