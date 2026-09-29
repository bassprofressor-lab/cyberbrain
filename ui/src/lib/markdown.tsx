/**
 * Markdown rendering with `[[wikilinks]]`. A link whose target exists navigates to the note;
 * one that does not is rendered as intent (dashed, muted, title explains) — never hidden,
 * never an error (SPEC §3.1).
 */
import { dict } from "@/lib/i18n";
import ReactMarkdown, { type Components, defaultUrlTransform } from "react-markdown";
import remarkGfm from "remark-gfm";
import type { Root, Text, Link, Parent } from "mdast";
import { href } from "./router";

const WIKI = /\[\[([^\]\n]+?)\]\]/g;

/** remark plugin: text `[[name]]` → link node with url `wiki:name`. Code nodes are not text nodes, so fences are untouched. */
function remarkWikilinks() {
  return (tree: Root) => {
    const walk = (node: Parent) => {
      for (let i = 0; i < node.children.length; i++) {
        const child = node.children[i];
        if (!child) continue;
        if (child.type === "text") {
          const text = child as Text;
          if (!WIKI.test(text.value)) {
            WIKI.lastIndex = 0;
            continue;
          }
          WIKI.lastIndex = 0;
          const out: Array<Text | Link> = [];
          let last = 0;
          for (const m of text.value.matchAll(WIKI)) {
            const at = m.index ?? 0;
            if (at > last) out.push({ type: "text", value: text.value.slice(last, at) });
            const name = (m[1] ?? "").trim();
            out.push({ type: "link", url: `wiki:${name}`, children: [{ type: "text", value: name }] });
            last = at + m[0].length;
          }
          if (last < text.value.length) out.push({ type: "text", value: text.value.slice(last) });
          node.children.splice(i, 1, ...out);
          i += out.length - 1;
        } else if ("children" in child) {
          walk(child as Parent);
        }
      }
    };
    walk(tree);
  };
}

/**
 * `wiki:` survives the URL sanitiser; everything else goes through it unchanged.
 *
 * react-markdown drops any protocol it does not know before a component sees the link, so every
 * `[[name]]` arrived at `a` with `href=""`: a note that existed was rendered as a link to
 * nowhere (2026-09-29). A `wiki:` URL never reaches the DOM as such — the `a` component below
 * turns it into the router's own address or into intent — so letting it through the sanitiser
 * opens nothing.
 */
function keepWiki(url: string): string {
  return url.startsWith("wiki:") ? url : defaultUrlTransform(url);
}

export interface MarkdownProps {
  source: string;
  /** Names that resolve to a note. Anything else in `[[…]]` is intent. */
  resolves: (name: string) => boolean;
  className?: string;
}

export function Markdown({ source, resolves, className }: MarkdownProps) {
  const components: Components = {
    // `node` is react-markdown's syntax-tree node. Spread into `rest` it ended up in the DOM
    // as `node="[object Object]"`.
    a: ({ node: _node, href: url, children, ...rest }) => {
      if (typeof url === "string" && url.startsWith("wiki:")) {
        const name = url.slice(5);
        if (resolves(name)) {
          return (
            <a href={href("note", name)} className="link" {...rest}>
              {children}
            </a>
          );
        }
        return (
          <span className="link-intent" title={dict().markdown.wikilinkIntent(name)} data-intent={name}>
            {children}
            <span aria-hidden className="ml-0.5 text-[0.7em] align-super">?</span>
          </span>
        );
      }
      // External links are real links: the browser opens them on the user's click, the page
      // itself fetches nothing. noopener/noreferrer so the target learns nothing about us.
      return (
        <a href={url} target="_blank" rel="noopener noreferrer" {...rest}>
          {children}
        </a>
      );
    },
    img: ({ alt, src }) => (
      <span className="inline-block px-2 py-1 rounded border border-dashed text-fg-muted text-xs" title={dict().markdown.imageTitle}>
        {dict().markdown.image}: {alt || (typeof src === "string" ? src : "")}
      </span>
    ),
  };
  return (
    <div className={`prose-note ${className ?? ""}`}>
      <ReactMarkdown remarkPlugins={[remarkGfm, remarkWikilinks]} components={components} urlTransform={keepWiki} skipHtml>
        {source}
      </ReactMarkdown>
    </div>
  );
}
