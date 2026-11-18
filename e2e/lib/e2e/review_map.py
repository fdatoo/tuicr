"""Local worktree and hand-authored v2 guide for the map scenarios."""

import json
from pathlib import Path

from . import repos
from .core import Scenario


def guide_fixture(s: Scenario, *, risky: bool = False, long_context: bool = False) -> tuple[Path, Path]:
    repo = repos.empty_repo(s.dir / "repo")
    paths = ("api", "bridge", "finish")
    for name in paths:
        rows = [
            f"{name} context {line:02}"
            + (" The incoming checkout arrives from the preview with a serialized postcode." if long_context else "")
            for line in range(1, 61)
        ]
        repos.write(repo / f"{name}.txt", "\n".join(rows) + "\n")
    repos.write(repo / "README.md", "# Review map fixture\n\nInitial description.\n")
    repos.git(repo, "add", "-A")
    repos.git(repo, "commit", "-qm", "base")
    for name in paths:
        path = repo / f"{name}.txt"
        lines = path.read_text().splitlines(keepends=True)
        lines[19:21] = [f"{name} change one\n", f"{name} change two\n"]
        repos.write(path, "".join(lines))
    repos.write(repo / "README.md", "# Review map fixture\n\nUpdated description.\n")
    digest = s.cli("guide", "outline", "-w", cwd=repo).splitlines()[0].removeprefix("diff: ")
    assert digest.startswith("sha256:"), digest

    bridge = {
        "id": "bridge",
        "claim": "Bridge forwards the request",
        "symbol": "fn forward_request()",
        "path": "bridge.txt",
        "start_line": 17,  # First line of the hunk; rows above it must come from the gap.
        "end_line": 21,
        "explanation": "The caller already chooses the route. This bridge hands the request to it.",
    }
    api: dict[str, object] = {
        "id": "api",
        "claim": "API returns the saved value",
        "symbol": "fn api_response()",
        "path": "api.txt",
        "start_line": 20,
        "end_line": 21,
    }
    if risky:
        reason = "This path used to return an error"
        if long_context:
            api["claim"] = (
                "API returns the saved value and keeps the existing lookup for overseas orders. "
                "The checkout reuses the quote it already showed, "
                "even when the caller returns to finish the order later."
            )
            reason = (
                "An absent postcode reaches the fallback used by overseas customers. "
                "The caller should not have to submit twice. Confirm the final contract."
            )
            api["explanation"] = (
                "The request falls back to its existing quote, using the old configuration. "
                "The caller sees the same checkout total that was calculated earlier. "
                "The receipt is not sent until the caller confirms the final amount."
                " The receipt and its total stay linked to the checkout that created them."
            )
        api["risk"] = {"kind": "error_handling", "reason": reason}
    finish = {
        "id": "finish",
        "claim": "Finish reports its outcome",
        "path": "finish.txt",
        "start_line": 20,
        "end_line": 21,
    }
    guide = repos.write(
        s.dir / "guide.json",
        json.dumps(
            {
                "version": 2,
                "diff": digest,
                "summary": "Follow a request from the bridge to the result.",
                "estimate_minutes": 12,
                "stories": [
                    {"title": "Request routing", "stops": [bridge, api]},
                    {"title": "Report the result", "stops": [finish]},
                ],
                "supporting": [
                    {
                        "label": "docs",
                        "basis": "mechanical",
                        "hunks": [{"path": "README.md", "old_start": 1, "new_start": 1}],
                    }
                ],
            }
        ),
    )
    return repo, guide
