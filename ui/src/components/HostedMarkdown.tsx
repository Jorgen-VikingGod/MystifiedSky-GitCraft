import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { hosting } from "@/lib/hosting";
import { useStore } from "@/lib/store";

function webUrl(input: string | undefined, base: string): URL | null {
  if (!input) return null;
  try {
    const url = new URL(input, base);
    return ["https:", "http:"].includes(url.protocol) && !url.username && !url.password ? url : null;
  } catch { return null; }
}

export function HostedMarkdown({ text, base }: { text: string; base: string }) {
  return (
    <div className="hosted-markdown text-[13px] text-fg break-words select-text">
      <Markdown skipHtml remarkPlugins={[remarkGfm]}
        urlTransform={value => webUrl(value, base)?.href ?? ""}
        components={{
          a: ({ href, children }) => href ? <a href={href} onClick={event => {
            event.preventDefault();
            void hosting.openUrl(href).catch(error => useStore.getState().toast(false, String(error)));
          }} onAuxClick={event => event.preventDefault()}>{children}</a> : <span>{children}</span>,
          img: ({ src, alt }) => {
            const url = webUrl(src, base);
            const allowed = url?.protocol === "https:" && (url.hostname === "gravatar.com" || url.hostname.endsWith(".gravatar.com"));
            return allowed ? <img src={url.href} alt={alt ?? ""} loading="lazy" referrerPolicy="no-referrer" />
              : <span className="text-fg3">{alt || "External image"}</span>;
          },
        }}
      >{text}</Markdown>
    </div>
  );
}
