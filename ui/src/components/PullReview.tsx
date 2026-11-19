import { useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { hosting, type PullRequest, type PullReviewAction } from "@/lib/hosting";
import { Button } from "./ui/button";
import { HostedMarkdown } from "./HostedMarkdown";

interface Line { side: "LEFT" | "RIGHT"; number: number; text: string; previous?: number }
export function reviewLines(patch: string): Line[] {
  const lines: Line[] = [];
  let left = 0, right = 0;
  for (const text of patch.split("\n")) {
    const range = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(text);
    if (range) {
      left = Number(range[1]);
      right = Number(range[2]);
    } else if (text.startsWith("+")) {
      lines.push({ side: "RIGHT", number: right++, text });
    } else if (text.startsWith("-")) {
      lines.push({ side: "LEFT", number: left++, text });
    } else if (text.startsWith(" ")) {
      lines.push({ side: "RIGHT", number: right++, text, previous: left++ });
    }
  }
  return lines;
}

export function PullReview({ path, request }: { path: string; request: PullRequest }) {
  const [open, setOpen] = useState(false);
  const [file, setFile] = useState("");
  const [selection, setSelection] = useState<Line | null>(null);
  const [review, setReview] = useState("");
  const [inline, setInline] = useState("");
  const [reviewers, setReviewers] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  const client = useQueryClient();
  const data = useQuery({ queryKey: ["pullReview", path, request.number, request.source_sha], enabled: open, retry: false,
    queryFn: ({ signal }) => hosting.reviewData(path, request.number, request.source_sha, signal) });
  const picked = data.data?.files.find(row => row.path === file);
  const lines = useMemo(() => reviewLines(picked?.patch ?? ""), [picked?.patch]);
  const isOpen = ["open", "opened"].includes(request.state);
  const submit = async (action: PullReviewAction, clear: () => void) => {
    if (busy) return;
    setBusy(true);
    setError("");
    setMessage("");
    try {
      await hosting.submitReview(path, request.number, request.source_sha, action);
      clear();
      setMessage("Review updated.");
      void client.invalidateQueries({ queryKey: ["pr", path, request.number] });
      void client.invalidateQueries({ queryKey: ["prsPages", path] });
      void data.refetch();
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  };
  const box = "block w-full bg-field rounded border border-line px-2 py-1.5 text-fg";
  return <details open={open} onToggle={event => setOpen(event.currentTarget.open)} className="mb-4 text-[12.5px]">
    <summary className="cursor-pointer text-fg2">Review</summary>
    {data.isLoading && <p role="status">Loading review…</p>}
    {data.isError && <p role="alert" className="text-bad">{String(data.error)} <button type="button" className="text-link" onClick={() => data.refetch()}>Retry</button></p>}
    {error && <p role="alert" className="text-bad">{error}</p>}
    {message && <p role="status" className="text-good">{message}</p>}
    {data.data && <div className="mt-2 space-y-3">
      <p className="text-fg2">{data.data.reviewers.length ? "Requested reviewers · " + data.data.reviewers.join(", ") : "No reviewers requested."}</p>
      {data.data.reviews.map(([author, state, body], index) => <div key={index}>
        <p className="text-fg2">{author} · {state.replaceAll("_", " ").toLowerCase()}</p>
        {body && <HostedMarkdown text={body} base={request.url} />}
      </div>)}
      {isOpen && <>
        <div className="flex gap-2">
          <input aria-label="Reviewer usernames" placeholder="Comma-separated usernames" className={box} value={reviewers} onChange={event => setReviewers(event.target.value)} />
          <Button size="sm" disabled={busy || !reviewers.trim()} onClick={() => submit({ kind: "requestReviewers", reviewers: [...new Set(reviewers.split(",").map(name => name.trim()).filter(Boolean))] }, () => setReviewers(""))}>Request review</Button>
        </div>
        <textarea aria-label="Review message" placeholder="Review message" className={box} value={review} onChange={event => setReview(event.target.value)} />
        <div className="flex gap-2">
          <Button size="sm" disabled={busy} onClick={() => submit({ kind: "approve", body: review }, () => setReview(""))}>Approve</Button>
          <Button size="sm" disabled={busy || !review.trim()} onClick={() => submit({ kind: "requestChanges", body: review }, () => setReview(""))}>Request changes</Button>
        </div>
      </>}
      {!!data.data.files.length && <label className="block">
        Changed file
        <select className={box} value={file} onChange={event => { setFile(event.target.value); setSelection(null); setInline(""); }}>
          <option value="">Choose a file</option>
          {data.data.files.map(row => <option key={row.path} value={row.path}>{row.path}</option>)}
        </select>
      </label>}
      {picked && <div className="max-h-52 overflow-auto border border-line mono">
        {lines.slice(0, 1500).map(line => <button type="button" key={line.side + line.number} aria-pressed={selection === line}
          title={isOpen ? "Select line for an inline comment" : undefined} disabled={!isOpen}
          onClick={() => setSelection(line)} className={"block w-full text-left whitespace-pre px-2 hover:bg-hoverrow " + (selection === line ? "bg-selrow" : "")}>
          {line.number} {line.text}
        </button>)}
        {lines.length > 1500 && <p className="p-2 text-fg3">Showing the first 1,500 diff lines. Open the pull request in your browser to review later lines.</p>}
      </div>}
      {isOpen && selection && <div>
        <label>Comment on line {selection.number} ({selection.side === "RIGHT" ? "new" : "old"})
          <textarea className={box} value={inline} onChange={event => setInline(event.target.value)} />
        </label>
        <Button size="sm" disabled={busy || !inline.trim()} onClick={() => submit({ kind: "inline", path: file, side: selection.side, line: selection.number, body: inline, old_line: selection.previous }, () => { setInline(""); setSelection(null); })}>Post inline comment</Button>
      </div>}
      {data.data.comments.filter(comment => !file || comment.path === file).map(comment => <div key={comment.id}>
        <p className="text-fg2">{comment.author} · {comment.path}{comment.line ? " · line " + comment.line : ""}</p>
        <HostedMarkdown text={comment.body} base={request.url} />
      </div>)}
    </div>}
  </details>;
}
