/** Share a small IPC budget across all agent worktree cards. Active reads keep
 * their slot until IPC settles, even if their consumer is cancelled. */
export class ReadScheduler {
  private active = 0;
  private queue: Array<() => void> = [];
  constructor(readonly limit = 4) {}
  run<T>(read: () => Promise<T>, signal?: AbortSignal): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      const abort = () => {
        const index = this.queue.indexOf(start);
        if (index >= 0) this.queue.splice(index, 1);
        reject(new DOMException("Read cancelled", "AbortError"));
      };
      const start = () => {
        signal?.removeEventListener("abort", abort);
        if (signal?.aborted) { abort(); return; }
        this.active++;
        Promise.resolve().then(read).then(resolve, reject).finally(() => { this.active--; this.drain(); });
      };
      if (signal?.aborted) { abort(); return; }
      signal?.addEventListener("abort", abort, { once: true });
      this.queue.push(start); this.drain();
    });
  }
  private drain() { while (this.active < this.limit && this.queue.length) this.queue.shift()!(); }
}
export const worktreeReads = new ReadScheduler();
