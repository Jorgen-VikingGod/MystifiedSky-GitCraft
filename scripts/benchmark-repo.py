"""Make a benchmark repository: python3 scripts/benchmark-repo.py <folder> <commits>.

Commits on main and short feature branches merged back now and then, written with
git fast-import, with one uncommitted change. 120 and 50000 commits are the usual sizes.

The folder must be new or empty, inside the current folder or the system's temporary folder.
"""
import os
import subprocess
import sys
import tempfile

MAX_COMMITS = 1_000_000


def target_folder(arg):
    """The folder to create the repository in, resolved and checked: inside the current folder or
    the temporary folder (no way out through `..` or links), and new or empty."""
    folder = os.path.realpath(arg)
    bases = [os.path.realpath(os.getcwd()), os.path.realpath(tempfile.gettempdir())]
    if not any(folder != base and folder.startswith(base + os.sep) for base in bases):
        sys.exit(f"The folder must be inside {bases[0]} or {bases[1]}.")
    if os.path.exists(folder) and (not os.path.isdir(folder) or os.listdir(folder)):
        sys.exit(f"{folder} must be a new or empty folder.")
    return folder


def commit_count(arg):
    try:
        n = int(arg)
    except ValueError:
        n = 0
    if not 1 <= n <= MAX_COMMITS:
        sys.exit(f"The number of commits must be between 1 and {MAX_COMMITS}.")
    return n


def history(n):
    """A fast-import stream of n commits."""
    lines = []
    t = 1600000000
    mark = 0
    heads = {"main": None}
    for i in range(n):
        branch = "main" if i == 0 or i % 7 else f"feature/{i % 40}"
        mark += 1
        parent = heads.get(branch) or heads["main"]
        lines.append(f"commit refs/heads/{branch}")
        lines.append(f"mark :{mark}")
        lines.append(f"author Dev{i%13} <dev{i%13}@example.invalid> {t+i*60} +0000")
        lines.append(f"committer Dev{i%13} <dev{i%13}@example.invalid> {t+i*60} +0000")
        msg = f"Change {i}: update module {i%97}"
        lines.append(f"data {len(msg.encode())}")
        lines.append(msg)
        if parent:
            lines.append(f"from {parent}")
        if branch == "main" and i % 50 == 0:
            for b, h in list(heads.items()):
                if b != "main" and h:
                    lines.append(f"merge {h}")
                    del heads[b]
                    break
        content = f"line {i}\n"
        lines.append(f"M 100644 inline src/mod{i%97}.txt")
        lines.append(f"data {len(content)}")
        lines.append(content)
        heads[branch] = f":{mark}"
    lines.append("done")
    return ("\n".join(lines) + "\n").encode()


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    folder, n = target_folder(sys.argv[1]), commit_count(sys.argv[2])
    os.makedirs(folder, exist_ok=True)
    # Git runs in the folder rather than being given its name, so no name reads as an option.
    git = lambda *args, **kw: subprocess.run(["git", *args], cwd=folder, check=True, **kw)
    git("init", "-q", "-b", "main")
    git("fast-import", "--quiet", "--done", input=history(n))
    git("checkout", "-q", "-f", "main")
    with open(os.path.join(folder, "src", "mod1.txt"), "a") as f:
        f.write("work in progress\n")


if __name__ == "__main__":
    main()
