import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { hosting } from "@/lib/hosting";
import { useStore } from "@/lib/store";

export function CommitChecks({ path, sha, number, remoteName, initiallyOpen = false }: { path: string; sha: string; number?: number; remoteName?: string; initiallyOpen?: boolean }) {
  const [open, setOpen] = useState(initiallyOpen);
  const checks = useQuery({ queryKey: ["commitChecks", path, sha, number, remoteName], enabled: open, staleTime: 30_000, retry: false,
    queryFn: ({ signal }) => hosting.checks(path, sha, number, signal, remoteName) });
  const openUrl = (value: string) => {
    try {
      const url = new URL(value);
      if (url.protocol === "https:" && !url.username && !url.password)
        void hosting.openUrl(url.href).catch(error => useStore.getState().toast(false, String(error)));
    } catch { /* The provider omitted a usable link. */ }
  };
  return <details open={open} onToggle={event => setOpen(event.currentTarget.open)} className="my-2 text-[12px] text-fg2">
    <summary className="cursor-pointer">Checks{checks.data?.length ? " (" + checks.data.length + ")" : ""}</summary>
    {checks.isLoading && <p role="status">Loading checks…</p>}
    {checks.isError && <p role="alert">{String(checks.error)}</p>}
    {checks.data?.length === 0 && <p>No checks were reported.</p>}
    {checks.data?.map((check, i) => <div key={i} className="flex items-center gap-2 py-0.5">
      <span className={check.state === "passed" ? "text-good" : check.state === "failed" ? "text-bad" : "text-fg3"}>{check.state}</span>
      {check.url ? <button type="button" onClick={() => openUrl(check.url)} className="text-link hover:underline truncate">{check.name}</button> : <span>{check.name}</span>}
    </div>)}
    {!checks.isLoading && <button className="text-link" disabled={checks.isFetching} onClick={() => checks.refetch()}>Refresh checks</button>}
  </details>;
}
