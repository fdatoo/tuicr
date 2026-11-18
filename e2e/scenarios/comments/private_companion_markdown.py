#!/usr/bin/env python3
# Fork: companion Markdown presentation, themed source and fence syntax, and raw private data.
import itertools
import json
import re

from e2e.companion import fake_claude

from e2e import Failure, Scenario, Tui, repos, scenario, wait_file

ANSWER = """## Packet summary

**Important**: [read the docs](https://example.invalid/) before changing `limit`.

- Check the caller
- Keep the return value intact

```Rust example
let fenced_value = 42;
```

Wide cells: 界界界界界界界界界界界界界界界界界界界界界界界界 👩‍💻

End of packet."""
DRAFT = """let prose_preview = 17;

**Draft advice**: keep `limit` within bounds.

```rust
let draft_value = 5;
```"""
SGR = re.compile(r"\x1b\[([;0-9]*)m")


def panel_line(text: str) -> str:
    return rf"\s*{re.escape(text)}\s*[│┃]?$"


def on_screen(tui: Tui, text: str) -> str:
    pattern = panel_line(text)
    tui.wait_for(pattern)
    screen = tui.screen(colors=True, join=False)
    body = screen.split("Private companion", 1)[1]
    for row in body.splitlines():
        if re.search(pattern, SGR.sub("", row)):
            return row
    raise Failure(f"no styled companion row matched {pattern}", screen)


def foreground_at(row: str, token: str) -> tuple[int, ...] | None:
    offset = SGR.sub("", row).index(token)
    foreground = None
    parts = SGR.split(row)
    for idx, part in enumerate(parts):
        if idx % 2:
            sgr = iter(int(value or "0") for value in part.split(";"))
            for code in sgr:
                if code in (0, 39):
                    foreground = None
                elif 30 <= code <= 37 or 90 <= code <= 97:
                    foreground = (code,)
                elif code in (38, 48, 58):
                    mode = next(sgr, 0)
                    count = 3 if mode == 2 else int(mode == 5)
                    channels = tuple(itertools.islice(sgr, count))
                    if code == 38:
                        foreground = (code, mode, *channels)
        elif offset < len(part):
            return foreground
        else:
            offset -= len(part)
    raise AssertionError(f"token not found in terminal row: {token!r}")


def assert_syntax(row: str, literal: str) -> tuple[int, ...]:
    keyword = foreground_at(row, "let")
    number = foreground_at(row, literal)
    assert keyword and number and keyword != number, f"keyword and number should have distinct colors: {row!r}"
    return keyword


@scenario
def main(s: Scenario) -> None:
    repo = repos.empty_repo(s.dir / "repo")
    path = repo / "source.rs"
    repos.write(path, "fn main() {\n    let old_value = 0;\n}\n")
    repos.git(repo, "add", "source.rs")
    repos.git(repo, "commit", "-qm", "base")
    repos.write(path, "fn main() {\n    let preview_value = 7;\n}\n")
    fake_claude(
        s,
        f"""
if 'Draft one concise' in prompt:
    assert '## Packet summary' in prompt and 'Retain the limit' in prompt
    answer({DRAFT!r})
elif 'explicitly requested more detail' in prompt:
    answer({ANSWER!r})
else:
    assert 'Inspect the packet' in prompt and 'let preview_value = 7;' in prompt
    answer('The caller owns the limit.')
""",
    )

    tui = s.start("--theme", "dark", cwd=repo, cols=110, rows=44)
    tui.wait_for("Unstaged changes").keys("Enter").cursor_on("preview_value")
    tui.cmd("ask").wait_for("Private companion")
    preview_dark = assert_syntax(on_screen(tui, "let preview_value = 7;"), "7;")
    tui.type("Inspect the packet").keys("Enter").wait_for("The caller owns the limit")
    tui.wait_gone("Streaming").keys(*(["C-Up"] * 7), "C-o")
    tui.wait_for(panel_line("Packet summary"))
    tui.wait_for(panel_line("Important: read the docs before changing limit."))
    tui.wait_for(panel_line("• Check the caller"))
    tui.wait_for(panel_line("• Keep the return value intact"))
    fence_dark = assert_syntax(on_screen(tui, "let fenced_value = 42;"), "42;")
    tui.wait_gone("Streaming")

    tui.paste("retained界").wait_for("Question: retained界")
    tui.keys("Tab").wait_for("Private review history").wait_for("More detail")
    tui.keys("Enter").wait_for("Packet summary").wait_for("Question: retained界")

    tui.cmd("theme light").cmd("ask").wait_for("Question: retained界")
    assert foreground_at(on_screen(tui, "let preview_value = 7;"), "let") != preview_dark
    assert foreground_at(on_screen(tui, "let fenced_value = 42;"), "let") != fence_dark

    tui.resize(60, 15).keys("C-u").paste("retained界\n👩‍💻last")
    tui.keys("Home", "Right", "BSpace", "!", "End", "?").wait_for(r"!last\?")
    tui.wait_for("Question: retained界")
    tui.resize(110, 44).keys("C-u").type("Retain the limit and request a test.")
    tui.keys("C-d").wait_for(panel_line("Draft advice: keep limit within bounds."))
    tui.wait_for("Feedback draft")
    assert_syntax(on_screen(tui, "let draft_value = 5;"), "5;")
    exported = s.dir / "review.md"
    tui.cmd(f"file {exported}").wait_for("No comments to write")
    assert not exported.exists(), "private Markdown must not be exported"
    records = list(s.dir.glob("**/companion/*/*.json"))
    assert len(records) == 2 and any(json.loads(record.read_text())["answer"] == ANSWER for record in records)

    tui.cmd("ask").wait_for(panel_line("Draft advice: keep limit within bounds."))
    tui.keys("C-e").wait_for("COMMENT").wait_for(r"\*\*Draft advice\*\*: keep `limit` within bounds\.")
    tui.keys("C-s").wait_for("NORMAL")
    comment = s.session()["files"]["source.rs"]["line_comments"]["2"][0]
    assert comment["content"] == DRAFT, comment
    tui.cmd(f"file {exported}")
    report = wait_file(exported).read_text()
    assert "**Draft advice**: keep `limit` within bounds." in report and "Packet summary" not in report

    tui.cursor_on("prose_preview").keys("a").wait_for("Private companion")
    prose_preview = on_screen(tui, "let prose_preview = 17;")
    assert foreground_at(prose_preview, "let") == foreground_at(prose_preview, "17;")
    tui.keys("Escape").wait_gone("Private companion").quit()
