#!/usr/bin/env python3
# Wheel/trackpad scrolling: keep direction changes, pane hit-testing, and an
# interleaved keyboard reset in order in both diff layouts.
from e2e import Scenario, repos, scenario


@scenario
def main(s: Scenario) -> None:
    repo = repos.empty_repo(s.dir / "repo")
    repos.git(repo, "commit", "--allow-empty", "-m", "base")
    repos.write(repo / "wheel.txt", "".join(f"wheel fixture {n:03d}\n" for n in range(1, 601)))
    repos.git(repo, "add", "wheel.txt")

    up = "\x1b[<64;70;6M"
    down = "\x1b[<65;70;6M"
    sidebar_down = "\x1b[<65;5;6M"
    for layout in ("unified", "side-by-side"):
        name = f"{s.name}_{layout}"
        repos.write(
            s.path_of(name) / "config" / "tuicr" / "config.toml",
            f'diff_view = "{layout}"\nshow_file_list = true\nscroll_offset = 0\nmouse_scroll_lines = 3\n',
        )
        tui = s.start("--working-tree", name=name, cwd=repo, keep=True, cols=120, rows=14)
        tui.wait_for("wheel fixture 001").click(70, 2)
        # Up at the top must clamp before Down is applied, not cancel it.
        tui.type("g" + up + down).wait_for("▶.*wheel fixture 002")
        # Reset inside the queued burst. The wheel over the sidebar must not
        # move the diff, even though it follows the same-direction diff wheels.
        tui.type(down + "g" + down * 8 + sidebar_down + up + down)
        tui.wait_for("▶.*wheel fixture 024")
        tui.quit()
