#!/usr/bin/env python3
# Review-guide rebuild: exports put one coverage line below the intro without changing numbered comments.
import shlex

from e2e.review_map import guide_fixture

from e2e import TUICR_BIN, Scenario, scenario


@scenario
def main(s: Scenario) -> None:
    repo, guide = guide_fixture(s)
    clip = s.fake_clipboard()
    for name, args in (("plain", []), ("guided", ["--guide", str(guide)])):
        stdout = s.dir / f"{name}-stdout.md"
        file = s.dir / f"{name}-file.md"
        command = shlex.join([TUICR_BIN, "--no-update-check", "--stdout", "-w", *args])
        tui = s.start_cmd(
            ["sh", "-c", f"exec {command} >{shlex.quote(str(stdout))}"],
            cwd=repo,
            name=f"{s.name}_{name}",
        )
        if args:
            tui.wait_for(r"Review (?:guide )?ready")
            tui.cmd("coverage").wait_for("Stops verified: 0/3").keys("Escape").wait_gone("Stops verified")
        tui.wait_for("api change one").cursor_on("20 ▌ api change one")
        tui.comment("A saved inline export comment")
        tui.cmd(f"file {file}").wait_for("Review written to")
        tui.cmd("clip").wait_exit()
        markdown = stdout.read_text()
        assert markdown == file.read_text(), markdown
        lines = markdown.splitlines()
        note = [line for line in lines if line.startswith("Reviewed with a guide:")]
        if args:
            intro = next(index for index, line in enumerate(lines) if line.startswith("I reviewed your code"))
            assert len(note) == 1 and lines[intro + 2] == note[0], markdown
            assert "verified 0 of 3 stops" in note[0], note[0]
        else:
            assert not note, markdown
        assert "1. `api.txt:20` - A saved inline export comment" in markdown, markdown
    assert not clip.copied, "--stdout export should not touch the clipboard"
