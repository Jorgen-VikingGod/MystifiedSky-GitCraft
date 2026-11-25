"""Launch, memory and scrolling benchmark for GitCraft builds (Linux; see TESTING.md).

    python3 scripts/benchmark.py <name> <runs> <repo> -- <gitcraft command...>
    python3 scripts/benchmark.py scroll <name> <frames> <repo> -- <gitcraft command...>

Every launch gets an empty HOME and a clean desktop environment. The build writes
`ready <unix ms>` to $GITCRAFT_BENCH once the first populated graph is drawn (the probe in
crates/gitcraft/src/app/bench.rs); the launch time is from starting the process to that line,
and memory (RSS, PSS and private, in MB) is summed over the process tree 3 s later, so a web
view's helper processes count too. The first launch is a warm-up; medians are printed as JSON.
`scroll` measures the CPU time of the whole process tree per frame while the build scrolls the
graph itself between its `mark scroll-start` and `mark scroll-end` lines.
$GCBENCH_SETTINGS (JSON) seeds the native build's preferences, e.g. '{"showAllCommits": true}'.
"""
import json, os, shutil, signal, subprocess, sys, tempfile, time

def tree(pid):
    out, todo = [], [pid]
    while todo:
        p = todo.pop()
        out.append(p)
        try:
            for t in os.listdir(f"/proc/{p}/task"):
                with open(f"/proc/{p}/task/{t}/children") as f:
                    todo += [int(c) for c in f.read().split()]
        except OSError:
            pass
    return out

def memory(pid):
    total = {"rss": 0, "pss": 0, "private": 0}
    for p in tree(pid):
        try:
            with open(f"/proc/{p}/smaps_rollup") as f:
                for line in f:
                    k, *v = line.split()
                    kb = int(v[0]) if v and v[0].isdigit() else 0
                    if k == "Rss:": total["rss"] += kb
                    elif k == "Pss:": total["pss"] += kb
                    elif k in ("Private_Clean:", "Private_Dirty:"): total["private"] += kb
        except OSError:
            pass
    return {k: round(v / 1024, 1) for k, v in total.items()}

KEEP = ("DISPLAY", "WAYLAND_DISPLAY", "XDG_RUNTIME_DIR", "XDG_SESSION_TYPE", "XDG_CURRENT_DESKTOP", "DBUS_SESSION_BUS_ADDRESS", "LANG", "XAUTHORITY")

def env_for(home, out):
    # A clean desktop session environment (no editor or snap variables), the same for every build.
    env = {k: os.environ[k] for k in KEEP if k in os.environ}
    env.update(HOME=home, PATH="/usr/local/bin:/usr/bin:/bin", GITCRAFT_BENCH=out, GITCRAFT_NO_WINDOW_RESTORE="1")
    return env

def launch(cmd, repo, extra=None, timeout=60):
    home = tempfile.mkdtemp(prefix="gcbench-")
    out = os.path.join(home, "bench.txt")
    # Preferences for the native build's isolated profile, from $GCBENCH_SETTINGS (JSON).
    if os.environ.get("GCBENCH_SETTINGS"):
        os.makedirs(os.path.join(home, "GitCraft"), exist_ok=True)
        with open(os.path.join(home, "GitCraft", "settings.json"), "w") as f:
            f.write(json.dumps({"settings": json.loads(os.environ["GCBENCH_SETTINGS"])}))
    env = env_for(home, out)
    env.update(extra or {})
    start = time.time() * 1000
    proc = subprocess.Popen(cmd + [repo], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
    return proc, out, start, home

def lines(out):
    try:
        with open(out) as f:
            return f.read().splitlines()
    except OSError:
        return []

def stop(proc, home):
    try:
        os.killpg(proc.pid, signal.SIGKILL)
    except OSError:
        pass
    proc.wait()
    shutil.rmtree(home, ignore_errors=True)

def startup(name, runs, repo, cmd):
    results = []
    for i in range(runs + 1):  # the first is a warm-up
        proc, out, start, home = launch(cmd, repo)
        deadline = time.time() + 60
        ready = None
        while time.time() < deadline and ready is None:
            for l in lines(out):
                if l.startswith("ready "):
                    ready = int(l.split()[1]) - start
            time.sleep(0.005)
        mem = None
        if ready is not None:
            time.sleep(3)
            mem = memory(proc.pid)
        stop(proc, home)
        if i > 0:
            results.append({"launch_ms": round(ready, 1) if ready is not None else None, **(mem or {})})
    ok = [r for r in results if r["launch_ms"] is not None]
    med = lambda k: sorted(r[k] for r in ok)[len(ok) // 2] if ok else None
    summary = {"name": name, "repo": os.path.basename(repo), "runs": results, "median": {k: med(k) for k in ("launch_ms", "rss", "pss", "private")}}
    print(json.dumps(summary))

def cpu_ticks(pid):
    """User + system CPU time of the process tree, in clock ticks."""
    total = 0
    for p in tree(pid):
        try:
            with open(f"/proc/{p}/stat") as f:
                fields = f.read().rsplit(")", 1)[1].split()
                total += sum(int(fields[i]) for i in (11, 12, 13, 14))  # utime, stime, cutime, cstime
        except (OSError, IndexError, ValueError):
            pass
    return total

def scroll(name, frames, repo, cmd, runs=3):
    """CPU time of the whole process tree per scrolled frame, between the app's scroll marks."""
    hz = os.sysconf("SC_CLK_TCK")
    results, details = [], []
    for _ in range(runs):
        proc, out, start, home = launch(cmd, repo, {"GITCRAFT_BENCH_SCROLL": str(frames)})
        deadline = time.time() + 300
        end = None
        while time.time() < deadline and end is None:
            if any(l.startswith("mark scroll-end") for l in lines(out)):
                end = True
            time.sleep(0.05)
        text = lines(out)
        details.append([l for l in text if not l.startswith("mark") and not l.startswith("ready")])
        stop(proc, home)
        # The app writes its own CPU time into each mark, so the phase is bracketed exactly even if the
        # runner was late to see the marks; a run whose marks lack it is dropped.
        def mark_at(step):
            for l in text:
                f = l.split()
                if f[:2] == ["mark", step] and len(f) >= 4 and f[3].startswith("cpu="):
                    return int(f[2]), int(f[3][4:])
            return None
        begin, finish = mark_at("scroll-start"), mark_at("scroll-end")
        if begin and finish:
            results.append({"cpu_ms_per_frame": round((finish[1] - begin[1]) * 1000 / hz / frames, 3), "wall_s": round((finish[0] - begin[0]) / 1000, 2)})
    print(json.dumps({"name": name, "repo": os.path.basename(repo), "frames": frames, "runs": results, "app": details[-1] if details else []}))

if __name__ == "__main__":
    args = sys.argv[1:]
    if "--" not in args or len(args[:args.index("--")]) < 3:
        sys.exit(__doc__)
    cmd = args[args.index("--") + 1:]
    args = args[:args.index("--")]
    if args[0] == "scroll":
        scroll(args[1], int(args[2]), args[3], cmd)
    else:
        startup(args[0], int(args[1]), args[2], cmd)
