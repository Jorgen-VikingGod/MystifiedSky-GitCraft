import { PullReview } from "./PullReview";
import { CommitChecks } from "./CommitChecks";
import { HostedMarkdown } from "./HostedMarkdown";
import { useShallow } from "zustand/react/shallow";
import { useInfiniteQuery, useQuery, useQueryClient } from "@tanstack/react-query";
import { ExternalLink, GitPullRequest, CircleDot, X } from "lucide-react";
import { useMemo, useState } from "react";
import type { RepoView } from "@/lib/api";
import { useStore } from "@/lib/store";
import { act, confirmThen, refresh } from "@/lib/repo";
import { branchFromIssue, hosting, issueIdentity } from "@/lib/hosting";
import { Button } from "./ui/button";
import { Avatar } from "./Icons";
import { timeSince } from "@/lib/utils";

function Since({ iso }: { iso: string }) {
  const units = useStore((s) => s.metrics.time_units);
  if (!iso) return null;
  const t = timeSince(Date.parse(iso) / 1000, units);
  return <span style={{ color: t.color }}>{t.text}</span>;
}

/** A pull request (merge request): description, comments, checkout, merge, comment. */
export function PullRequestView({ repo, tabId, number }: { repo: RepoView; tabId: string; number: number }) {
  const s = useStore(useShallow((st) => ({ setUi: st.setUi, toast: st.toast })));
  const qc = useQueryClient();
  const prs = useQuery({ queryKey: ["pr", repo.path, number], queryFn: ({ signal }) => hosting.pr(repo.path, number, signal), staleTime: 60_000 });
  const pr = prs.data;
  const remote = useQuery({queryKey:["remote",repo.path],queryFn:({ signal })=>hosting.remote(repo.path, signal),staleTime:60_000});
  const comments = useInfiniteQuery({ queryKey: ["prCommentsPages", repo.path, number], initialPageParam: null as string | null,
    queryFn: ({pageParam, signal}) => hosting.commentsPage(repo.path, number, pageParam, signal), getNextPageParam: page => page.cursor ?? undefined });
  const commentRows = useMemo(() => [...new Map((comments.data?.pages.flatMap(page => page.items) ?? []).map(c => [c.id || JSON.stringify([c.author,c.created,c.body]), c])).values()], [comments.data]);
  const [text, setText] = useState("");
  const [method, setMethod] = useState("");
  const [squashMessage, setSquashMessage] = useState("");
  const [merged, setMerged] = useState<{ number: number; source: string; sha: string } | null>(null);
  const [remoteDeleted, setRemoteDeleted] = useState(false);
  const [busy, setBusy] = useState(false);
  const [merging, setMerging] = useState(false);
  const methods = pr?.merge_methods ?? (remote.data?.service === "bitbucket-server" ? ["default"] : []);
  const selectedMethod = methods.includes(method) ? method : methods[0] ?? "";
  const labels: Record<string, string> = {
    default: "Server default", unsquashed: "Merge without squashing", merge: "Merge commit", squash: "Squash and merge",
    rebase: "Rebase and merge", fast_forward: "Fast-forward", rebase_merge: "Rebase with merge commit",
  };
  const methodLabel = selectedMethod === "default" ? "server default strategy" : labels[selectedMethod];
  const mergeStatus = pr?.merge_status || (pr?.draft ? "Draft pull requests cannot be merged." : null);
  const close = () => s.setUi(tabId, { pr: null });
  const checkout = async () => {
    setBusy(true);
    try { await hosting.checkoutPr(repo.path, number); }
    catch (error) { s.toast(false, String(error)); }
    finally { setBusy(false); }
  };

  const send = async () => {
    if (!text.trim()) return;
    setBusy(true);
    try {
      await hosting.comment(repo.path, number, text.trim());
      setText("");
      comments.refetch();
    } catch (e) {
      s.toast(false, String(e));
    } finally {
      setBusy(false);
    }
  };
  const merge = () =>
    confirmThen(`Merge #${number}?`, `This merges ${pr?.source} into ${pr?.target} on the server (${methodLabel}).`, "Merge", async () => {
      setBusy(true); setMerging(true);
      try {
        await hosting.merge(repo.path, number, selectedMethod, pr?.source_sha || undefined, selectedMethod === "squash" ? squashMessage.trim() || undefined : undefined);
        if (pr?.source_sha) setMerged({ number, source: pr.source, sha: pr.source_sha });
        s.toast(true, `Merged #${number}`);
        qc.invalidateQueries({ queryKey: ["prs", repo.path] });
        qc.invalidateQueries({ queryKey: ["prsPages", repo.path] });
        qc.invalidateQueries({ queryKey: ["launchpadPages"] });
        qc.invalidateQueries({ queryKey: ["pr", repo.path, number] });
        void act(repo.path, { type: "fetch" }, true);
      } catch (e) {
        s.toast(false, String(e));
      } finally { setBusy(false); setMerging(false); }
    });

  const mergedSource = merged?.number === number ? merged
    : pr?.source_sha && ["merged", "completed"].includes(pr.state.toLowerCase()) ? { number, source: pr.source, sha: pr.source_sha } : null;
  const localSource = mergedSource && repo.head_branch !== mergedSource.source &&
    repo.refs.some(ref => ref.kind === "Local" && ref.name === mergedSource.source && ref.sha === mergedSource.sha);
  const deleteSource = (fromRemote: boolean) => {
    if (!mergedSource) return;
    const where = fromRemote ? "the source repository" : "this clone";
    confirmThen("Delete merged branch?", "Delete " + mergedSource.source + " from " + where + " if it still matches the merged commit?", "Delete Branch", async () => {
      setBusy(true);
      try {
        await hosting.deleteMergedSource(repo.path, number, mergedSource.sha, fromRemote);
        if (fromRemote) {
          setRemoteDeleted(true);
          await act(repo.path, { type: "fetch" }, true);
        } else await refresh(repo.path);
        s.toast(true, "Deleted " + mergedSource.source + " from " + where);
      } catch (error) { s.toast(false, String(error)); }
      finally { setBusy(false); }
    });
  };

  return (
    <div className="h-full flex flex-col bg-app min-w-0">
      <div className="flex items-center gap-2 h-[32px] pl-3 pr-2 bg-toolbar/60 border-b border-black/30 shrink-0">
        <GitPullRequest size={14} className="text-good" />
        <span className="text-[13px] text-white font-semibold truncate flex-1">
          #{number} {pr?.title ?? ""}
        </span>
        {pr && (
          <button className="text-fg2 hover:text-white p-1" title="Open in browser" onClick={() => hosting.openUrl(pr.url)}>
            <ExternalLink size={14} />
          </button>
        )}
        <button onClick={close} className="text-fg2 hover:text-white p-1" title="Close (Esc)">
          <X size={16} />
        </button>
      </div>
      {!pr && <div className="p-4 text-fg3 text-[13px]">{prs.isLoading ? "Loading…" : prs.isError ? String(prs.error) : "This pull request is no longer open."}</div>}
      {pr && (
        <div className="flex-1 overflow-auto p-4">
          <div className="flex flex-wrap items-center gap-2 text-[12.5px] text-fg2 mb-3">
            <Avatar name={pr.author} email={pr.author} size={20} />
            <span className="text-white">{pr.author}</span> wants to merge
            <span className="mono px-1.5 rounded bg-white/10 text-white">{pr.source}</span> into
            <span className="mono px-1.5 rounded bg-white/10 text-white">{pr.target}</span>
            {pr.draft && <span className="px-1.5 rounded bg-white/10 text-fg">Draft</span>}
            <span>
              · updated <Since iso={pr.updated} />
            </span>
          </div>
          <div className="flex flex-wrap gap-2 mb-4">
            {merging && method === "rebase" && remote.data?.provider === "gitlab" && <Button size="sm" onClick={() => hosting.cancelMerge(repo.path, number).catch(error => s.toast(false, String(error)))} title="Stop waiting before the merge is sent. The server rebase can still finish.">Cancel waiting</Button>}
            <Button size="sm" disabled={busy} onClick={checkout} title="Check out this pull request’s exact source commit in detached HEAD mode">
              Checkout {pr.source}
            </Button>
            {!mergedSource && !!methods.length && <select aria-label="Merge method" disabled={busy} value={selectedMethod} onChange={(e) => setMethod(e.target.value)} className="h-7 px-1 rounded-[4px] bg-field border border-white/10 text-[12.5px] text-fg outline-none">
              {methods.map(method => <option key={method} value={method}>{labels[method]}</option>)}
            </select>}
            {!mergedSource && <Button size="sm" variant="success" disabled={busy || !!mergeStatus || !selectedMethod} onClick={merge} title={mergeStatus ?? undefined}>
              Merge Pull Request
            </Button>}
            <Button size="sm" disabled={busy || prs.isFetching} onClick={() => void prs.refetch()}>Refresh merge status</Button>
            {mergedSource && <>
              {!remoteDeleted && <Button size="sm" disabled={busy} onClick={() => deleteSource(true)}>Delete remote source branch</Button>}
              {localSource && <Button size="sm" disabled={busy} onClick={() => deleteSource(false)}>Delete local source branch</Button>}
            </>}
          </div>
          {!mergedSource && selectedMethod === "squash" && !mergeStatus && <label className="block mb-3 text-[12.5px] text-fg2">
            Squash commit message
            <textarea value={squashMessage} onChange={event => setSquashMessage(event.target.value)} maxLength={16000}
              className="mt-1 w-full min-h-14 bg-field border border-line rounded px-2 py-1 text-fg" placeholder="Leave blank to use the server default" disabled={busy} />
          </label>}
          {mergedSource && <div role="status" className="mb-3 text-[12.5px] text-good">Merged</div>}
          {!mergedSource && (mergeStatus || !selectedMethod) && <div className="mb-3 text-[12.5px] text-fg2">{mergeStatus ?? "The hosting service did not provide an available merge method. Check it in the browser."}</div>}
          <div className="rounded border border-white/10 bg-panel p-3 text-[13px] text-fg whitespace-pre-wrap break-words mb-4 select-text">{pr.body ? <HostedMarkdown text={pr.body} base={pr.url} /> : <span className="text-fg3">No description.</span>}</div>
          {pr.source_sha && <CommitChecks key={pr.source_sha} path={repo.path} sha={pr.source_sha} number={number} />}
          {pr.source_sha && ["github", "github-enterprise", "gitlab", "gitlab-self"].includes(remote.data?.service ?? "") && <PullReview key={pr.source_sha} path={repo.path} request={pr} />}
          <div className="text-[12px] font-semibold tracking-wider text-fg2 mb-2">COMMENTS</div>
          {comments.isLoading && <div className="text-fg3 text-[12.5px]">Loading comments…</div>}
          {!comments.isLoading && !comments.isError && commentRows.length === 0 && <div className="text-fg3 text-[12.5px] mb-2">No comments yet.</div>}
          {commentRows.map((c, i) => (
            <div key={i} className="flex gap-2 mb-3">
              <Avatar name={c.author} email={c.author} size={24} />
              <div className="flex-1 min-w-0">
                <div className="text-[12px] text-fg2">
                  <span className="text-white">{c.author}</span> · <Since iso={c.created} />
                </div>
                <div className="text-[13px] text-fg whitespace-pre-wrap break-words select-text"><HostedMarkdown text={c.body} base={pr.url} /></div>
              </div>
            </div>
          ))}
          {comments.error && <div role="alert" className="text-bad">{String(comments.error)} <button onClick={() => comments.isFetchNextPageError ? comments.fetchNextPage() : comments.refetch()}>Retry comments</button> <button onClick={() => comments.refetch()}>Refresh comments</button></div>}
          {comments.hasNextPage && <button className="text-link mb-3" disabled={comments.isFetching} onClick={() => comments.fetchNextPage()}>Load more comments</button>}
          <textarea
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder="Leave a comment"
            className="w-full h-[80px] bg-field border border-white/10 rounded-[3px] px-2 py-1.5 text-[13px] text-white outline-none focus:border-accent resize-none"
          />
          <Button size="sm" variant="primary" className="mt-2" disabled={busy || !text.trim()} onClick={send}>
            Comment
          </Button>
        </div>
      )}
    </div>
  );
}

/** An issue: description, labels, create a branch for it. */
export function IssueView({ repo, tabId, number }: { repo: RepoView; tabId: string; number: string | number }) {
  const s = useStore(useShallow((st) => ({ setUi: st.setUi })));
  const issues = useQuery({ queryKey: ["issues", repo.path], queryFn: ({ signal }) => hosting.issues(repo.path, signal), staleTime: 60_000 });
  const issue = typeof number === "string" ? issues.data?.find((i) => issueIdentity(i) === number) : issues.data?.filter((i) => i.number === number).filter((_, _index, all) => all.length === 1)[0];
  return (
    <div className="h-full flex flex-col bg-app min-w-0">
      <div className="flex items-center gap-2 h-[32px] pl-3 pr-2 bg-toolbar/60 border-b border-black/30 shrink-0">
        <CircleDot size={14} className="text-good" />
        <span className="text-[13px] text-white font-semibold truncate flex-1">
          {issue?.key || (issue ? "#" + issue.number : "Issue")} {issue?.title ?? ""}
        </span>
        {issue && (
          <button className="text-fg2 hover:text-white p-1" title="Open in browser" onClick={() => hosting.openUrl(issue.url)}>
            <ExternalLink size={14} />
          </button>
        )}
        <button onClick={() => s.setUi(tabId, { issue: null })} className="text-fg2 hover:text-white p-1" title="Close (Esc)">
          <X size={16} />
        </button>
      </div>
      {!issue && <div className="p-4 text-fg3 text-[13px]">{issues.isLoading ? "Loading…" : "This issue is no longer open."}</div>}
      {issue && (
        <div className="flex-1 overflow-auto p-4">
          <div className="flex flex-wrap items-center gap-2 text-[12.5px] text-fg2 mb-3">
            opened by <span className="text-white">{issue.author}</span>
            {issue.labels.map((l) => (
              <span key={l} className="px-1.5 rounded bg-accent/20 text-white text-[11.5px]">
                {l}
              </span>
            ))}
          </div>
          <Button size="sm" variant="primary" className="mb-4" onClick={() => branchFromIssue(repo.path, issue)}>
            Create branch for this issue
          </Button>
          <div className="rounded border border-white/10 bg-panel p-3 text-[13px] text-fg whitespace-pre-wrap break-words select-text">{issue.body ? <HostedMarkdown text={issue.body} base={issue.url} /> : <span className="text-fg3">No description.</span>}</div>
          {["github", "gitlab", "bitbucket", "gitea", "forgejo", "codeberg"].includes(issue.tracker ?? "") && <IssueComments key={issue.number} path={repo.path} number={issue.number} url={issue.url} />}
        </div>
      )}
    </div>
  );
}

function IssueComments({ path, number, url }: { path: string; number: number; url: string }) {
  const comments = useInfiniteQuery({ queryKey: ["issueComments", path, number, url], initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }) => hosting.issueCommentsPage(path, number, pageParam, signal),
    getNextPageParam: page => page.cursor ?? undefined });
  const rows = useMemo(() => [...new Map((comments.data?.pages.flatMap(page => page.items) ?? []).map(row => [row.id, row])).values()], [comments.data]);
  return <div className="mt-4 text-[12px]">
    <div className="font-semibold tracking-wider text-fg2 mb-2">COMMENTS</div>
    {comments.isLoading && <p className="text-fg3">Loading comments…</p>}
    {comments.isSuccess && !rows.length && <p className="text-fg3">No comments yet.</p>}
    {rows.map(comment => <div className="mb-3" key={comment.id}>
      <p className="text-fg2">{comment.author} · <Since iso={comment.created} /></p>
      <HostedMarkdown text={comment.body} base={url} />
    </div>)}
    {comments.error && <div role="alert" className="text-bad">{String(comments.error)} <button onClick={() => comments.isFetchNextPageError ? comments.fetchNextPage() : comments.refetch()}>Retry comments</button></div>}
    {comments.hasNextPage && <button className="text-link" disabled={comments.isFetchingNextPage} onClick={() => comments.fetchNextPage()}>Load more comments</button>}
  </div>;
}
