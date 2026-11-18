/** Serialize each path's lifecycle so late registration cannot revive a closed tab. */
export class WatchManager {
  private entries = new Map<string, { desired: boolean; active: boolean; pending: boolean; attempts: number; timer?: ReturnType<typeof setTimeout> }>();
  constructor(private watch: (path: string) => Promise<unknown>, private unwatch: (path: string) => Promise<unknown>, private failed: (path: string, error: unknown) => void, private closed: (path: string) => void) {}
  update(paths: Iterable<string>) {
    const wanted = new Set(paths);
    for (const [path, entry] of this.entries) {
      if (!wanted.has(path) && entry.desired) { entry.desired = false; clearTimeout(entry.timer); entry.timer = undefined; this.closed(path); }
    }
    for (const path of wanted) {
      const entry = this.entries.get(path);
      if (entry) { if (!entry.desired) entry.attempts = 0; entry.desired = true; }
      else this.entries.set(path, { desired: true, active: false, pending: false, attempts: 0 });
    }
    for (const path of this.entries.keys()) void this.reconcile(path);
  }
  private async reconcile(path: string) {
    const entry = this.entries.get(path);
    if (!entry || entry.pending || entry.timer) return;
    if (entry.desired === entry.active) { if (!entry.desired) this.entries.delete(path); return; }
    if (entry.desired && entry.attempts >= 4) return;
    entry.pending = true;
    const registering = entry.desired;
    try {
      if (registering) { entry.attempts++; await this.watch(path); entry.active = true; }
      else { await this.unwatch(path); entry.active = false; }
    } catch (error) {
      this.failed(path, error);
      if (registering && entry.desired && entry.attempts < 4) {
        entry.timer = setTimeout(() => { entry.timer = undefined; void this.reconcile(path); }, 1000 * 2 ** (entry.attempts - 1));
      }
      // A failed removal may still have a native watch. Keep it active for a later update.
      if (!registering) return;
    } finally { entry.pending = false; }
    if (entry.desired !== entry.active && !(registering && entry.desired)) void this.reconcile(path);
    else if (!entry.desired && !entry.active) this.entries.delete(path);
  }
}
