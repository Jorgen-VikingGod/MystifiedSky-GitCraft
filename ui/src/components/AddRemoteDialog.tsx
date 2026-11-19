import { useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { Cloud, Github, Gitlab, Globe, Layers, LoaderCircle } from "lucide-react";
import { integrations } from "@/lib/integrations";
import { act, queryClient, refresh, repoData } from "@/lib/repo";
import { useStore } from "@/lib/store";
import { cn } from "@/lib/utils";
import { Dialog, Field, inputClass } from "./ui/dialog";
import { Button } from "./ui/button";

const providers = [
  { id: "url", label: "URL", icon: Globe },
  { id: "github", label: "GitHub", icon: Github },
  { id: "gitlab", label: "GitLab", icon: Gitlab },
  { id: "bitbucket", label: "Bitbucket", icon: Layers },
  { id: "gitea", label: "Gitea", icon: Cloud },
  { id: "forgejo", label: "Forgejo", icon: Cloud },
  { id: "codeberg", label: "Codeberg", icon: Cloud },
  { id: "azure-server", label: "Azure Server", icon: Cloud },
  { id: "azure", label: "Azure DevOps", icon: Cloud },
  { id: "github-enterprise", label: "GitHub Enterprise", icon: Github },
  { id: "gitlab-self", label: "Self-hosted GitLab", icon: Gitlab },
];
const serviceIds = providers.slice(1).map(p => p.id);
interface Target { id: string; label: string }
interface PublishRequest { service: string; target: string; name: string; description: string; visibility: string; remote: string; branch: string; head: string }
interface PublishResult { url: string; connected: boolean; pushed: boolean; error: string | null }

export function AddRemoteDialog({ path, onClose }: { path: string; onClose: () => void }) {
  const settings = useStore(s => s.settings);
  const [repo] = useState(() => repoData(path));
  const [service, setService] = useState("url");
  const [name, setName] = useState(repo?.name ?? path.split(/[\\/]/).pop() ?? "");
  const [remote, setRemote] = useState("origin");
  const [description, setDescription] = useState("");
  const [visibility, setVisibility] = useState("private");
  const [url, setUrl] = useState("");
  const [pushUrl, setPushUrl] = useState("");
  const [urlConnected, setUrlConnected] = useState(false);
  const [selected, setSelected] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const running = useRef(false);
  const [error, setError] = useState("");
  const [created, setCreated] = useState<{ request: PublishRequest; result: PublishResult } | null>(null);
  const statuses = useQuery({ queryKey: ["integrationStatuses", "publish"], queryFn: () => integrations.statuses(serviceIds) });
  const status = statuses.data?.find(s => s.id === service);
  const targets = useQuery({ queryKey: ["publishTargets", service, status?.account], queryFn: () => invoke<Target[]>("publish_targets", { service }), enabled: service !== "url" && !!status?.connected });
  const target = targets.data?.find(destination => destination.id === selected[service])?.id ?? targets.data?.[0]?.id ?? "";
  const supportsInternal = service === "gitlab-self" || ((service === "github" || service === "github-enterprise") && target.startsWith("org:"));
  const access = visibility === "internal" && !supportsInternal ? "private" : visibility;
  const providerConfiguration: Record<string, string> = {
    "github-enterprise": settings.integrations.githubEnterpriseHost,
    "gitlab-self": settings.integrations.gitlabHost || settings.gitlabHost,
    "azure-server": settings.integrations.azureServerCollection,
    gitea: settings.integrations.giteaHost,
    forgejo: settings.integrations.forgejoHost,
  };
  const visibleProviders = providers.filter(provider => !(provider.id in providerConfiguration) ||
    providerConfiguration[provider.id] || provider.id === service || statuses.data?.some(status => status.id === provider.id && !!status.host));
  const azure = service === "azure" || service === "azure-server";
  const label = providers.find(p => p.id === service)!.label;
  const ready = remote.trim() && (service === "url" ? url.trim() : status?.connected && target && name.trim() && repo?.head_branch && repo.head_sha);
  const close = () => { if (!running.current) onClose(); };
  const work = async (run: () => Promise<void>) => {
    if (running.current) return;
    running.current = true; setBusy(true); setError("");
    try { await run(); } catch (e) { setError(String(e)); }
    finally { running.current = false; setBusy(false); }
  };
  const publish = async () => {
    const request = created?.request ?? { service, target, name: name.trim(), description: azure ? "" : description.trim(), visibility: azure ? "private" : access, remote: remote.trim(), branch: repo!.head_branch!, head: repo!.head_sha };
    const result = await invoke<PublishResult>(created ? "resume_publish" : "publish_repository", { path, request, ...(created ? { url: created.result.url } : {}) });
    setCreated({ request, result });
    await refresh(path);
    if (result.error) setError(result.error);
    else if (result.pushed) { useStore.getState().toast(true, `Created remote and pushed ${request.branch}`); onClose(); }
  };
  const addUrl = async () => {
    if (!urlConnected && !await act(path, { type: "addRemote", name: remote.trim(), url: url.trim() }, true)) { setError("Could not add the remote. Check the error notification and retry."); return; }
    setUrlConnected(true);
    // Retain the newly added URL if a later optional setting fails; never repeat remote creation.
    if (pushUrl.trim() && !await act(path, { type: "remoteEdit", name: remote.trim(), newName: "", url: url.trim(), pushUrl: pushUrl.trim() }, true)) {
      setError("The remote was added, but the push URL could not be saved. Edit the remote to finish configuring it."); return;
    }
    if (!await act(path, { type: "fetchRemote", remote: remote.trim() }, true)) { setError("The remote was added, but fetching failed. Retry to fetch it again."); return; }
    onClose();
  };
  const connect = () => work(async () => {
    await integrations.signIn(service, "browser");
    await queryClient.invalidateQueries({ queryKey: ["integrationStatuses"] });
    await queryClient.invalidateQueries({ queryKey: ["signedInServices"] });
    await queryClient.invalidateQueries({ queryKey: ["publishTargets", service] });
  });
  const renderBody = () => {
    if (created) return <div className="space-y-3 text-[13px]">
        <p>The hosted repository was created{created.result.connected ? " and connected" : ""}. {created.result.pushed ? "The branch was pushed." : "Publishing is not finished."}</p>
        <Field label="Repository URL"><input className={inputClass} value={created.result.url} readOnly/></Field>
        <p className="text-fg2 text-xs">Retry uses this repository. It will not create another one.</p>
        <Button className="w-full" variant="primary" disabled={busy} onClick={() => void work(publish)}>Retry connection and push</Button>
      </div>;
    if (service === "url") return <form onSubmit={e => { e.preventDefault(); if (ready) void work(addUrl); }}>
        <fieldset disabled={busy}>
          <legend className="sr-only">Existing remote</legend>
          <Field label="Name"><input className={inputClass} value={remote} onChange={e => setRemote(e.target.value)} required readOnly={urlConnected}/></Field>
          <Field label="Pull URL"><input className={inputClass} placeholder="https://github.com/owner/project.git" value={url} onChange={e => setUrl(e.target.value)} required readOnly={urlConnected}/></Field>
          <Field label="Push URL (optional)"><input className={inputClass} value={pushUrl} onChange={e => setPushUrl(e.target.value)}/></Field>
          <Button className="w-full" variant="primary" disabled={!ready || busy} type="submit">{urlConnected ? "Retry remote setup" : "Add Remote"}</Button>
        </fieldset>
      </form>;
    if (statuses.isPending) return <p role="status" className="text-fg2 text-xs">Checking connected accounts…</p>;
    if (!status?.connected) return <div className="py-6 text-center text-[13px] space-y-3">
        <p>{label} is not connected</p>
        {(status?.error || statuses.error) && <p role="alert" className="text-bad text-xs">{status?.error ?? String(statuses.error)}</p>}
        {service === "azure" && !settings.integrations.azureOrg ? <Button onClick={() => useStore.getState().setDialog({ kind: "prefs", section: "integrations" })}>Configure Azure DevOps</Button> : <Button variant="primary" disabled={busy} onClick={connect}>Connect to {label}</Button>}
        <Button onClick={() => useStore.getState().setDialog({ kind: "prefs", section: "integrations" })}>Integration settings</Button>
      </div>;
    return <form onSubmit={e => { e.preventDefault(); if (ready) void work(publish); }}>
        <fieldset disabled={busy}>
          <legend className="sr-only">Hosted repository</legend>
          <Field label="Account"><select className={inputClass} value={status.account ?? ""} disabled><option value={status.account ?? ""}>{status.account}</option></select></Field>
          {<Field label={azure ? "Project" : service === "bitbucket" ? "Workspace" : "Destination"}><select className={inputClass} value={target} onChange={e => setSelected(s => ({ ...s, [service]: e.target.value }))}><option value="" disabled>Select…</option>{targets.data?.map(t => <option key={t.id} value={t.id}>{t.label}</option>)}</select></Field>}
          {targets.isPending && <p role="status" className="text-fg2 text-xs mb-3">Loading publishing destination…</p>}
          {targets.error && <div role="alert" className="text-bad text-xs mb-3">{String(targets.error)} <Button type="button" onClick={() => void targets.refetch()}>Retry</Button></div>}
          {targets.data?.length === 0 && <p role="alert" className="text-fg2 text-xs mb-3">No publishing destination is available for this account.</p>}
          <Field label="Repository Name"><input className={inputClass} value={name} onChange={e => setName(e.target.value)} required pattern="[A-Za-z0-9_.-]+"/></Field>
          <Field label="Remote Name"><input className={inputClass} value={remote} onChange={e => setRemote(e.target.value)} required/></Field>
          {!azure && <><Field label="Description"><input className={inputClass} value={description} onChange={e => setDescription(e.target.value)}/></Field>
          <Field label="Access"><select className={inputClass} value={access} onChange={e => setVisibility(e.target.value)}><option value="private">Private</option><option value="public">Public</option>{supportsInternal && <option value="internal">Internal (if supported)</option>}</select></Field></>}
          {azure && <p className="text-fg2 text-xs mb-3">Access is inherited from the selected Azure project.</p>}
          <p className="text-fg2 text-xs mb-3">Creates an empty repository and pushes the current branch, {repo?.head_branch ?? "unavailable"}, with its committed history.</p>
          {!repo?.head_branch || !repo.head_sha ? <p role="alert" className="text-bad text-xs mb-3">Check out a branch with at least one commit before publishing.</p> : null}
          <Button className="w-full" variant="primary" disabled={!ready || busy || targets.isFetching || !!targets.error} type="submit">Create remote and push branch</Button>
        </fieldset>
      </form>;
  };
  return <Dialog open onClose={close} title="Add Remote" width={500} drawerLeft={settings.showLeft ? settings.leftWidth + 4 : 16}>
    <div className="flex flex-wrap border-b border-line -mx-4 -mt-1 mb-5 px-3" role="tablist" aria-label="Remote source">
      {visibleProviders.map((p, i) => <button key={p.id} role="tab" tabIndex={service === p.id ? 0 : -1} aria-selected={service === p.id} aria-controls="remote-source-panel" id={`remote-tab-${p.id}`} disabled={busy || !!created || urlConnected} onKeyDown={e => {
        const indexes: Record<string, number> = { ArrowRight: (i + 1) % visibleProviders.length, ArrowLeft: (i + visibleProviders.length - 1) % visibleProviders.length, Home: 0, End: visibleProviders.length - 1 };
        if (e.key in indexes) { e.preventDefault(); const next = visibleProviders[indexes[e.key]].id; setService(next); setError(""); document.getElementById(`remote-tab-${next}`)?.focus(); }
      }} onClick={() => { setService(p.id); setError(""); }} className={cn("flex-1 flex flex-col items-center gap-2 py-3 text-[11px] border-b-2", service === p.id ? "text-fg border-accent" : "text-fg3 border-transparent hover:text-fg")}><p.icon size={25}/>{p.label}</button>)}
    </div>
    <div role="tabpanel" id="remote-source-panel" aria-labelledby={`remote-tab-${service}`}>
      {busy && <p role="status" className="flex items-center gap-2 text-fg2 text-xs mb-3"><LoaderCircle size={14} className="animate-spin"/>Working…</p>}
      {error && <p role="alert" className="text-bad text-xs whitespace-pre-wrap mb-3">{error}</p>}
      {renderBody()}
    </div>
    <div className="mt-4 flex justify-end"><Button disabled={busy} onClick={close}>Cancel</Button></div>
  </Dialog>;
}
