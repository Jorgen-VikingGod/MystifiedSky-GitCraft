import { create } from "zustand";
import { invokeRead } from "./api";

const requests = new Set<AbortController>();
export const useAiRequests = create<{ count: number }>(() => ({ count: 0 }));
export function stopAiRequests() {
  requests.forEach((request) => request.abort());
}
import { DEFAULT_AI, useStore } from "./store";

/** Bind every operation to its own connection snapshot, including connection tests. */
export async function invokeAi<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const config = { ...DEFAULT_AI, ...useStore.getState().settings.ai };
  const request = new AbortController();
  requests.add(request);
  useAiRequests.setState({ count: requests.size });
  try {
    return await invokeRead<T>(command, { ...args, config }, request.signal);
  } finally {
    requests.delete(request);
    useAiRequests.setState({ count: requests.size });
  }
}
