#!/usr/bin/env python3
# Fork review guide: a visual range stays selected and its preview remains bounded.
from e2e.companion import fake_claude

from e2e import Scenario, repos, scenario


def companion_panel(screen: str) -> str:
    return screen.split("Private companion", 1)[1]


@scenario
def main(s: Scenario) -> None:
    repo = repos.empty_repo(s.dir / "repo")
    path = repo / "preview.txt"
    repos.write(path, "initial line\n")
    repos.git(repo, "add", "preview.txt")
    repos.git(repo, "commit", "-qm", "base")
    repos.write(path, "".join(f"anchor-preview-{n:02d}\n" for n in range(1, 17)))
    repos.write(s.dir / "config/tuicr/config.toml", 'leader = "j"\n')
    fake_claude(
        s,
        """
assert 'Review anchor: preview.txt:1–6 (new)' in prompt
assert 'Selected code' in prompt and 'anchor-preview-01' in prompt and 'anchor-preview-06' in prompt
answer('This explains the selected range.')
""",
    )
    tui = s.start(cwd=repo, cols=68, rows=35, keep=True)
    tui.wait_for("Unstaged changes").keys("Enter").wait_for("anchor-preview-01")
    tui.cmd("1").keys("v", "j", "j", "j", "j", "j", "a")
    tui.wait_for("Private companion .*preview.txt:1–6")
    tui.type("Why these lines?").wait_for(r"Question: Why these lines\?")
    panel = companion_panel(tui.screen(join=False))
    assert "anchor-preview-01" in panel and "(truncated)" in panel, panel
    assert "anchor-preview-06" not in panel, panel
    tui.keys("Enter").wait_for("This explains the selected range.")

    tui.keys("Escape").wait_gone("Private companion")
    assert "anchor-preview-06" in tui.cursor_row(), tui.screen()
    tui.keys("a").wait_for("Private companion .*preview.txt:1–6")
    tui.wait_for("This explains the selected range.")
    tui.keys("Escape").wait_gone("Private companion").wait_for("VISUAL")
    tui.keys("Escape").wait_for("NORMAL").keys(":q!", "Enter").wait_exit()
