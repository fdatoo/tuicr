"""Synthetic review and provider fixtures for preparation CLI scenarios."""

import json
from pathlib import Path
from typing import Any

from . import repos
from .core import E2E_DIR, TOOL_PATH, Scenario


def review_fixture(s: Scenario) -> tuple[Path, dict[str, Any]]:
    repo = repos.empty_repo(s.dir / "repo")
    repos.write(repo / "README.md", "# Demo\n")
    repos.git(repo, "add", "README.md")
    repos.git(repo, "commit", "-qm", "base")
    repos.write(repo / "reviewed.rs", 'fn greeting() -> &\'static str {\n    "hello"\n}\n')
    repos.git(repo, "add", "reviewed.rs")
    outline = s.cli("guide", "outline", "-w", cwd=repo)
    prefix, digest = outline.splitlines()[0].split(": ", maxsplit=1)
    assert prefix == "diff" and len(digest.removeprefix("sha256:")) == 64 and digest.startswith("sha256:"), outline
    stop = {
        "id": "greeting",
        "claim": "The greeting is a static string.",
        "explanation": "A caller asks for a greeting. This function returns a literal rather than allocating.",
        "path": "reviewed.rs",
        "side": "new",
        "start_line": 1,
        "end_line": 3,
        "symbol": "greeting",
        "untested": True,
        "followups": [
            {"question": "Where is the string created?", "answer": "It is a literal in the source."},
            {"question": "Does every caller get the same greeting?", "answer": "Yes, it takes no inputs."},
        ],
    }
    guide = {
        "version": 2,
        "diff": digest,
        "summary": "Add a small greeting function.",
        "stories": [{"title": "Introduce a greeting", "stops": [stop]}],
        "estimate_minutes": 2,
    }
    return repo, guide


def github_pr_response() -> dict[str, Any]:
    stop = {
        "id": "greeting",
        "claim": "The greeting prints hello from the PR.",
        "explanation": "The new module exposes a greeting function. Calling it prints the greeting to stdout.",
        "path": "src/greet.rs",
        "start_line": 1,
        "end_line": 3,
        "untested": True,
    }
    return {
        "guide": {
            "version": 2,
            "diff": "",
            "summary": "Expose a greeting function and update the README.",
            "stories": [{"title": "Introduce a greeting", "stops": [stop]}],
            "estimate_minutes": 12,
        },
        "audit": {"stops": [{"id": "greeting", "audit": {"result": "holds"}, "untested": True}]},
    }


def fake_claude(
    s: Scenario,
    *,
    response: dict[str, Any] | None,
    requires: tuple[str, ...] = (),
    forbids: tuple[str, ...] = (),
    digest_from_prompt: bool = False,
    release: Path | None = None,
) -> Path:
    calls = s.dir / "claude-calls"
    answer = None if response is None else json.dumps(response)
    await_release = (
        f"release = Path({str(release)!r})\nwhile not release.exists(): time.sleep(0.03)\n"
        if release is not None
        else ""
    )
    replace_digest = (
        "guide = json.loads(answer)\n"
        "digest = re.search(r'Diff digest: (sha256:[0-9a-f]{64})', prompt)\n"
        "assert digest is not None, 'prompt omitted diff digest'\n"
        "guide['guide']['diff'] = digest[1]\n"
        "answer = json.dumps(guide)\n"
        if digest_from_prompt
        else ""
    )
    client = repos.write(
        s.dir / "bin/claude",
        "#!/usr/bin/env python3\n"
        "import json, re, sys, time\n"
        "from pathlib import Path\n"
        "prompt = sys.stdin.read()\n"
        f"with Path({str(calls)!r}).open('a') as calls: calls.write('called\\n')\n"
        f"{await_release}"
        f"assert all(s in prompt for s in {requires!r}), 'missing preparation context'\n"
        f"assert not any(s in prompt for s in {forbids!r}), 'wrong preparation phase'\n"
        f"answer = {answer!r}\n"
        "if answer is None:\n"
        "    print(json.dumps({'type': 'system', 'subtype': 'init'}))\n"
        "    print(prompt, file=sys.stderr)\n"
        "    sys.exit(23)\n"
        f"{replace_digest}"
        "print(json.dumps({'type': 'system', 'subtype': 'init', 'tools': ['Read', 'Glob', 'Grep']}))\n"
        "result = 'Result:\\n```json\\n' + answer + '\\n```'\n"
        "print(json.dumps({'type': 'result', 'subtype': 'success', 'result': result}))\n",
    )
    client.chmod(0o755)
    s.env.update(TUICR_COMPANION="claude", PATH=f"{client.parent}:{E2E_DIR}/bin:{TOOL_PATH}")
    return calls


def stored_state_paths(s: Scenario) -> list[Path]:
    return [p for p in s.path_of(s.name).rglob("state.json") if p.parent.parent.name == "guides"]


def stored_state_path(s: Scenario) -> Path:
    root = s.path_of(s.name)
    paths = stored_state_paths(s)
    assert len(paths) == 1, f"Expected one preparation state under {root}, got {paths}"
    return paths[0]
