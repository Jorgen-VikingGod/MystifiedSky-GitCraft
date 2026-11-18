import { afterEach, expect, test, vi } from "vitest";
import { WatchManager } from "./watch-manager";
import { ReadScheduler } from "./read-scheduler";
const flush = async () => { for (let i = 0; i < 10; i++) await Promise.resolve(); };
afterEach(() => vi.useRealTimers());
test("failed registration retries, successful tabs do not duplicate watches", async () => {
  vi.useFakeTimers();
  const watch = vi.fn().mockRejectedValueOnce(new Error("offline")).mockResolvedValue(undefined);
  const failed = vi.fn(); const manager = new WatchManager(watch, vi.fn().mockResolvedValue(undefined), failed, vi.fn());
  manager.update(["repo"]); await flush(); expect(failed).toHaveBeenCalledTimes(1);
  manager.update(["repo"]); expect(watch).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(1000); expect(watch).toHaveBeenCalledTimes(2);
  manager.update(["repo"]); expect(watch).toHaveBeenCalledTimes(2); manager.update([]); await flush();
});
test("retries are bounded and reopening starts a fresh lifecycle", async () => {
  vi.useFakeTimers(); const watch = vi.fn().mockRejectedValue(new Error("failure"));
  const manager = new WatchManager(watch, vi.fn().mockResolvedValue(undefined), vi.fn(), vi.fn());
  manager.update(["repo"]); await vi.runAllTimersAsync(); expect(watch).toHaveBeenCalledTimes(4);
  manager.update([]); manager.update(["repo"]); await flush(); expect(watch).toHaveBeenCalledTimes(5); manager.update([]);
});
test("late registration is removed and close/reopen serializes", async () => {
  let resolve!: () => void; let stop!: () => void;
  const watch = vi.fn(() => new Promise<void>(r => { resolve = r; }));
  const unwatch = vi.fn(() => new Promise<void>(r => { stop = r; }));
  const manager = new WatchManager(watch, unwatch, vi.fn(), vi.fn());
  manager.update(["repo"]); manager.update([]); resolve(); await flush(); expect(unwatch).toHaveBeenCalledTimes(1);
  manager.update(["repo"]); expect(watch).toHaveBeenCalledTimes(1);
  stop(); await flush(); expect(watch).toHaveBeenCalledTimes(2);
  manager.update([]); resolve(); await flush(); stop(); await flush(); expect(unwatch).toHaveBeenCalledTimes(2);
});
test("status scheduling caps peak reads and drops cancelled queued reads", async () => {
  const scheduler = new ReadScheduler(4); const controller = new AbortController();
  let active = 0, peak = 0; const release: Array<() => void> = [];
  const read = vi.fn(() => new Promise<void>(resolve => { active++; peak = Math.max(peak, active); release.push(() => { active--; resolve(); }); }));
  const tasks = Array.from({length: 30}, () => scheduler.run(read, controller.signal).catch(() => undefined));
  await flush(); expect(read).toHaveBeenCalledTimes(4); expect(peak).toBe(4);
  controller.abort(); release.forEach(r => r()); await Promise.all(tasks); await flush(); expect(read).toHaveBeenCalledTimes(4);
});
