#!/usr/bin/env python3
# Review-guide rebuild: a long stop cannot squeeze the Ask transcript off screen.
import json

from e2e import Scenario, repos, scenario


def dock_rows(screen: str) -> tuple[list[str], list[str]]:
    rows = screen.splitlines()
    title = next(i for i, row in enumerate(rows) if "Private companion" in row)
    return rows[:title], rows[title:]


@scenario
def main(s: Scenario) -> None:
    repo = repos.empty_repo(s.dir / "repo")
    repos.write(repo / "README.md", "# Review fixture\n")
    repos.git(repo, "add", "README.md")
    repos.git(repo, "commit", "-qm", "base")
    source = "".join(f"long-stop-code-{line:02}\n" for line in range(1, 46))
    repos.write(repo / "handler.txt", source)
    repos.git(repo, "add", "handler.txt")
    digest = s.cli("guide", "outline", "-w", cwd=repo).splitlines()[0].removeprefix("diff: ")
    explanation = "\n\n".join(f"Prepared paragraph {i:02} explains the caller." for i in range(1, 18))
    stop = {
        "id": "handler",
        "claim": "The handler keeps the transaction together.",
        "path": "handler.txt",
        "start_line": 1,
        "end_line": 45,
        "explanation": explanation,
        "followups": [{"question": "Where does it start?", "answer": "At the handler."}],
    }
    guide = repos.write(
        s.dir / "guide.json",
        json.dumps({"version": 2, "diff": digest, "stories": [{"title": "Handler", "stops": [stop]}]}),
    )
    tui = s.start("-w", "--guide", guide, cwd=repo, cols=105, rows=55)
    tui.wait_for(r"\. start").keys(".").wait_for(r"▶.*long-stop-code-01")
    tui.keys("a").wait_for("Private companion").wait_for("Question:")
    code, dock = dock_rows(tui.screen(join=False))
    for line in range(1, 7):
        assert any(f"long-stop-code-{line:02}" in row for row in code), "\n".join(code)
    assert not any("long-stop-code-45" in row for row in code)
    # Track and thumb mark the actual transcript area, excluding chrome.
    assert sum(row.endswith(("│", "┃")) for row in dock) >= 10, "\n".join(dock)
    thumb = [i for i, row in enumerate(dock) if row.endswith("┃")]
    assert thumb, "\n".join(dock)
    tui.keys("PageUp", "PageUp", "PageUp").wait_for("Prepared paragraph 01")
    _, scrolled = dock_rows(tui.screen(join=False))
    assert thumb != [i for i, row in enumerate(scrolled) if row.endswith("┃")]
    tui.keys("S-Down", "S-Down").wait_gone("long-stop-code-01")
    tui.keys("C-Up").settle()
    _, grown = dock_rows(tui.screen(join=False))
    assert len(grown) > len(dock)
    tui.keys("Escape").wait_gone("Private companion").keys("a").wait_for("Private companion")
    _, reopened = dock_rows(tui.screen(join=False))
    assert len(reopened) == len(grown), "the dock resize should survive closing and reopening"
