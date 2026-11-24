"""Make a benchmark repository: python3 scripts/benchmark-repo.py <folder> <commits>.

Commits on main and short feature branches merged back now and then, written with
git fast-import, with one uncommitted change. 120 and 50000 commits are the usual sizes.
"""
import subprocess, sys, os
path, n = sys.argv[1], int(sys.argv[2])
os.makedirs(path, exist_ok=True)
subprocess.run(["git", "init", "-q", "-b", "main", path], check=True)
lines = []
t = 1600000000
mark = 0
heads = {"main": None}
for i in range(n):
    branch = "main" if i % 7 else f"feature/{i % 40}"
    mark += 1
    parent = heads.get(branch) or heads["main"]
    lines.append(f"commit refs/heads/{branch}")
    lines.append(f"mark :{mark}")
    lines.append(f"author Dev{i%13} <dev{i%13}@example.invalid> {t+i*60} +0000")
    lines.append(f"committer Dev{i%13} <dev{i%13}@example.invalid> {t+i*60} +0000")
    msg = f"Change {i}: update module {i%97}"
    lines.append(f"data {len(msg.encode())}"); lines.append(msg)
    if parent: lines.append(f"from {parent}")
    if branch == "main" and i % 50 == 0:
        for b, h in list(heads.items()):
            if b != "main" and h:
                lines.append(f"merge {h}"); del heads[b]; break
    content = f"line {i}\n"
    lines.append(f"M 100644 inline src/mod{i%97}.txt"); lines.append(f"data {len(content)}"); lines.append(content)
    heads[branch] = f":{mark}"
lines.append("done")
subprocess.run(["git", "-C", path, "fast-import", "--quiet", "--done"], input=("\n".join(lines)+"\n").encode(), check=True)
subprocess.run(["git", "-C", path, "checkout", "-q", "-f", "main"], check=True)
with open(os.path.join(path, "src", "mod1.txt"), "a") as f: f.write("work in progress\n")
