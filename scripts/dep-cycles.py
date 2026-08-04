#!/usr/bin/env python3
"""Report module dependency cycles inside a crate.

Rust forbids crate cycles, so a cycle between modules of one crate is a boundary you
cannot draw later — it is the thing that blocks extracting either module into its own
crate. This finds them.

    python3 scripts/dep-cycles.py                      # defaults to crates/voxel-rt/src
    python3 scripts/dep-cycles.py crates/voxel-rt/src crates/core/src
    python3 scripts/dep-cycles.py --raw                # do NOT strip comments/tests

Exit code is 1 when a cycle is found, so this works as a check.

## Why the stripping matters

`--raw` counts `crate::foo` inside doc comments and `#[cfg(test)]` blocks. Neither is a
production dependency, and both inflate the picture badly: on `voxel-rt` they turned one
real 10-module cycle into an apparent 32-module tangle (214 edges instead of 143), which
reads as "this cannot be untangled" rather than "one type is in the wrong module". Default
is stripped; `--raw` exists to show the difference.

Resolution is by module path, so `crate::passes::dda` resolves to the `passes::dda` module
rather than to `passes`. It only counts targets that exist as files, so `crate::SomeType`
reexported from `lib.rs` is correctly ignored.
"""

from __future__ import annotations

import argparse
import pathlib
import re
import sys

CRATE_PATH = re.compile(r"crate::([a-z_][a-z_0-9]*(?:::[a-z_][a-z_0-9]*)*)")
LINE_COMMENT = re.compile(r"^[ \t]*//.*$", re.M)
BLOCK_COMMENT = re.compile(r"/\*.*?\*/", re.S)
TEST_MOD = re.compile(r"#\[cfg\(test\)\]\s*(?:pub\s+)?mod\s+\w+\s*\{")


def strip_noise(source: str) -> str:
    """Remove comments and `#[cfg(test)] mod … { … }` blocks."""
    source = LINE_COMMENT.sub("", source)
    source = BLOCK_COMMENT.sub("", source)

    kept: list[str] = []
    cursor = 0
    while True:
        found = TEST_MOD.search(source, cursor)
        if not found:
            kept.append(source[cursor:])
            return "".join(kept)
        kept.append(source[cursor : found.start()])
        # Walk from the opening brace to its match.
        depth = 0
        index = found.end() - 1
        while index < len(source):
            if source[index] == "{":
                depth += 1
            elif source[index] == "}":
                depth -= 1
                if depth == 0:
                    break
            index += 1
        cursor = index + 1


def module_name(path: pathlib.Path, root: pathlib.Path) -> str:
    relative = path.relative_to(root).with_suffix("")
    parts = list(relative.parts)
    if parts[-1] == "mod":
        parts.pop()
    return "::".join(parts)


def build_graph(root: pathlib.Path, raw: bool) -> dict[str, set[str]]:
    sources: dict[str, str] = {}
    for path in sorted(root.rglob("*.rs")):
        name = module_name(path, root)
        if name in ("", "main", "lib"):
            continue
        text = path.read_text(encoding="utf-8")
        sources[name] = text if raw else strip_noise(text)

    graph: dict[str, set[str]] = {name: set() for name in sources}
    for name, text in sources.items():
        for match in CRATE_PATH.finditer(text):
            parts = match.group(1).split("::")
            # Longest existing module path wins, so `passes::dda` beats `passes`.
            for length in range(len(parts), 0, -1):
                candidate = "::".join(parts[:length])
                if candidate in sources:
                    if candidate != name:
                        graph[name].add(candidate)
                    break
    return graph


def strongly_connected(graph: dict[str, set[str]]) -> list[list[str]]:
    """Tarjan, iterative — module counts are small but recursion depth is not worth risking."""
    index: dict[str, int] = {}
    low: dict[str, int] = {}
    on_stack: set[str] = set()
    stack: list[str] = []
    components: list[list[str]] = []
    counter = 0

    for start in graph:
        if start in index:
            continue
        work: list[tuple[str, list[str]]] = [(start, sorted(graph[start]))]
        index[start] = low[start] = counter
        counter += 1
        stack.append(start)
        on_stack.add(start)

        while work:
            node, pending = work[-1]
            if pending:
                child = pending.pop()
                if child not in index:
                    index[child] = low[child] = counter
                    counter += 1
                    stack.append(child)
                    on_stack.add(child)
                    work.append((child, sorted(graph[child])))
                elif child in on_stack:
                    low[node] = min(low[node], index[child])
            else:
                work.pop()
                if work:
                    low[work[-1][0]] = min(low[work[-1][0]], low[node])
                if low[node] == index[node]:
                    component = []
                    while True:
                        member = stack.pop()
                        on_stack.discard(member)
                        component.append(member)
                        if member == node:
                            break
                    components.append(component)
    return components


def report(root: pathlib.Path, raw: bool) -> int:
    graph = build_graph(root, raw)
    edges = sum(len(targets) for targets in graph.values())
    mode = "RAW (comments + tests counted)" if raw else "production (comments + tests stripped)"
    print(f"{root}: {len(graph)} modules, {edges} edges — {mode}")

    cycles = [sorted(component) for component in strongly_connected(graph) if len(component) > 1]
    if not cycles:
        print("  no cycles — every module is extractable once its dependencies are")
        return 0

    for cycle in sorted(cycles, key=len, reverse=True):
        members = set(cycle)
        print(f"\n  CYCLE [{len(cycle)} modules]: {', '.join(cycle)}")
        for member in cycle:
            inner = sorted(target for target in graph[member] if target in members)
            if inner:
                print(f"    {member:28} -> {', '.join(inner)}")
    print(f"\n{len(cycles)} cycle(s). Look for one type in the wrong module before redesigning.")
    return 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument(
        "roots",
        nargs="*",
        default=["crates/voxel-rt/src"],
        type=pathlib.Path,
        help="crate src directories to analyse (default: crates/voxel-rt/src)",
    )
    parser.add_argument(
        "--raw",
        action="store_true",
        help="count doc comments and #[cfg(test)] blocks too (shows why stripping matters)",
    )
    arguments = parser.parse_args()

    status = 0
    # argparse does not apply `type` to a list default, so normalise here.
    for root in (pathlib.Path(entry) for entry in arguments.roots):
        if not root.is_dir():
            print(f"{root}: not a directory", file=sys.stderr)
            status = 2
            continue
        status |= report(root, arguments.raw)
        print()
    return status


if __name__ == "__main__":
    sys.exit(main())
