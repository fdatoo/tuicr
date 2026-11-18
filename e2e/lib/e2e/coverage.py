"""Hand-written review guides for the fake GitHub fixture, without a provider."""

import json

from . import repos
from .core import Scenario
from .tui import Tui

PRIVATE_ANSWER = "This private guide answer must not be posted."


def guided_pr(s: Scenario, *, risky: bool = False) -> Tui:
    repo = repos.pr_repo(s.dir / "repo")
    env = {"E2E_VARIANT": "threads_and_commits", "TUICR_PREPARE_ON_OPEN": "0"}
    digest = s.cli("guide", "outline", "pr", "1", cwd=repo, env=env).splitlines()[0].removeprefix("diff: ")
    assert digest.startswith("sha256:"), digest
    readme = {
        "id": "docs",
        "path": "README.md",
        "start_line": 2,
        "end_line": 3,
        "claim": "The README describes the new example.",
        "explanation": "The example introduces the greeting output before the reader opens the Rust module.",
        "followups": [{"question": "What does the example lead to?", "answer": PRIVATE_ANSWER}],
    }
    greeting: dict[str, object] = {
        "id": "greeting",
        "path": "src/greet.rs",
        "start_line": 1,
        "end_line": 3,
        "claim": "The greeting prints a single line.",
    }
    if risky:
        greeting["risk"] = {"kind": "unexplained", "reason": "This path is used in unattended runs."}
    guide = {
        "version": 2,
        "diff": digest,
        "stories": [
            {"title": "Document the example", "stops": [readme]},
            {"title": "Print a greeting", "stops": [greeting]},
        ],
        "estimate_minutes": 22,
    }
    supplied = repos.write(s.dir / "guide.json", json.dumps(guide))
    return s.start("pr", "1", "--guide", supplied, cwd=repo, env=env).wait_for(r"Review (?:guide )?ready: 2 stops")
