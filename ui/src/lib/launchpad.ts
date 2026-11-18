import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { useStore } from "./store";
import { integrations, SERVICES } from "./integrations";
import { hosting, issueIdentity } from "./hosting";
import { ReadScheduler } from "./read-scheduler";
const reads = new ReadScheduler(4);

/** Providers arrive independently, with a fixed hook order across account changes. */
export function useLaunchpad(enabled = true) {
  const config = useStore((s) => s.settings.integrations);
  const accounts = useQuery({ queryKey: ["integrationStatuses", "launchpad", config], queryFn: () => integrations.statuses(SERVICES.map(s => s.id)), enabled, staleTime: 60_000 });
  function useService(service: string) {
    return useInfiniteQuery({ queryKey: ["launchpadPages", service, config, accounts.data?.find(s => s.id === service)?.account], initialPageParam: null as string | null,
      queryFn: ({pageParam, signal}) => reads.run(() => hosting.launchpadPage(service, pageParam, signal), signal),
      getNextPageParam: page => page.cursor ?? undefined,
      enabled: enabled && !!accounts.data?.some(s => s.id === service && s.connected && !s.error),
      staleTime: 60_000, refetchInterval: 5 * 60_000, retry: false });
  }
  const queries = [useService("github"), useService("github-enterprise"), useService("gitlab"), useService("gitlab-self"), useService("bitbucket"), useService("bitbucket-server"), useService("azure"), useService("jira"), useService("jira-server"), useService("trello")];
  const active = queries.filter((_, index) => enabled && accounts.data?.some(s => s.id === SERVICES[index].id && s.connected && !s.error));
  const pages = active.flatMap(q => q.data?.pages ?? []);
  const prs = [...new Map(pages.flatMap(p => p.items.pull_requests).map(p => [p.url || `${p.repo}#${p.number}`, p])).values()].sort((a,b) => b.updated.localeCompare(a.updated));
  const issues = [...new Map(pages.flatMap(p => p.items.issues).map(i => [issueIdentity(i), i])).values()].sort((a,b) => b.updated.localeCompare(a.updated));
  const errors = active.flatMap(q => q.error ? [String(q.error)] : q.data?.pages.at(-1)?.items.errors ?? []);
  if (accounts.error) errors.push(String(accounts.error));
  return { data: pages.length ? { pull_requests: prs, issues, errors } : undefined, errors,
    isFetching: accounts.isFetching || queries.some(q => q.isFetching),
    isLoading: accounts.isLoading || active.some(q => q.isLoading),
    hasNextPage: active.some(q => q.hasNextPage),
    connected: !!accounts.data?.some(s => s.connected && !s.error),
    refetch: () => Promise.all([accounts.refetch(), ...active.map(q => q.refetch())]),
    loadMore: () => Promise.all(active.filter(q => q.hasNextPage && !q.isFetching).map(q => q.fetchNextPage())) };
}
