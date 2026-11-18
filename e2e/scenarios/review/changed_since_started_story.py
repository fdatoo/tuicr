#!/usr/bin/env python3
# Review-guide rebuild: response edits form the first story and need explicit verdicts.
from e2e.review_map import guide_fixture

from e2e import Scenario, repos, scenario


@scenario
def main(s: Scenario) -> None:
    repo, guide = guide_fixture(s)
    api = repo / "api.txt"
    tui = s.start("-w", "--guide", guide, cwd=repo, keep=True)
    tui.wait_for("Review ready").cursor_on("api change one").comment("Handle the empty result")
    tui.cmd("guide").wait_for(r"▶.*bridge context 17").keys("K").wait_for(r"▶.*api change one")

    repos.write(api, api.read_text().replace("api change one", "api fixed after feedback"))
    tui.cmd("e").wait_for("Changed since you started").wait_for(r"Earlier version · :prepare")
    tui.cmd("version").wait_for(r"tuicr v\d").wait_for(r"Earlier version · :prepare")
    tui.cmd("changes").wait_for(r"▶.*api fixed after feedback").wait_for("Handle the empty result")
    tui.keys(".").wait_for("Explicit verdict needed").wait_for(r"▶.*api fixed after feedback")
    tui.keys("K").wait_for(r"▶.*finish change one")
    tui.cmd("guide off").cmd("guide").wait_for(r"▶.*finish change one")
    tui.quit()

    # A separate review with no guide uses the same stops and controls via :changes.
    other = repos.git_repo(s.dir / "without-guide")
    file = other / "README.md"
    simple = s.start("-w", cwd=other, name=f"{s.name}_standalone")
    simple.wait_for("unstaged line")
    repos.write(file, file.read_text().replace("unstaged line", "follow-up without a guide"))
    simple.cmd("e").wait_for(":changes to review")
    simple.cmd("changes").wait_for(r"▶.*follow-up without a guide")
    simple.keys(".").wait_for("Explicit verdict needed").keys("K").wait_for("Stories finished")
    simple.quit()
