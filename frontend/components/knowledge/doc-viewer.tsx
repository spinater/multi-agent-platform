"use client";

/** One indexed document in a modal (`#knowledge/<slug>/doc/<path>`): its facts, the card's `sources:` as links, and
 * the body through the safe Markdown renderer. Relative links to other indexed docs open them here; http(s) links
 * open in a new tab; anything else stays text. */

import { X } from "lucide-react";
import { useEffect, useMemo, useState, type ReactNode } from "react";

import { api } from "@/lib/api";
import type { KnowledgeDocBody } from "@/lib/generated/KnowledgeDocBody";
import type { KnowledgeSettings } from "@/lib/generated/KnowledgeSettings";

import { ago, errText } from "../project-ui";
import { Modal } from "../shell/page";
import { Badge } from "../ui";
import { fmtBytes } from "../branches";
import { Markdown, splitFrontmatter, type LinkRender } from "./markdown";
import { isMarkdown, KIND_LABEL, KIND_TONE, knowledgeHash, resolvePath } from "./shared";

type Loaded = { path: string; body: KnowledgeDocBody } | { path: string; error: string };

export function DocViewer({
  slug,
  path,
  known,
  settings,
  now,
  onOpen,
  onClose,
}: {
  slug: string;
  path: string;
  /** Paths of every indexed doc: only these become in-viewer links. */
  known: Set<string>;
  settings: KnowledgeSettings;
  now: number;
  onOpen: (path: string) => void;
  onClose: () => void;
}) {
  const [loaded, setLoaded] = useState<Loaded | null>(null);

  useEffect(() => {
    let live = true;
    api.knowledgeDoc(slug, path).then(
      (body) => live && setLoaded({ path, body }),
      (e) => live && setLoaded({ path, error: errText(e) }),
    );
    return () => {
      live = false;
    };
  }, [slug, path]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const ours = loaded?.path === path ? loaded : null;
  const data = ours && "body" in ours ? ours.body : null;
  const parts = useMemo(() => splitFrontmatter(data?.body ?? ""), [data]);

  /** An indexed doc a link or source names, tried relative to this doc, then to the project folder. */
  const target = (href: string): string | null => {
    const bare = href.replace(/[#?].*$/, "");
    let clean = bare;
    try {
      clean = decodeURIComponent(bare);
    } catch {
      // A stray `%`: use the link as written.
    }
    if (!clean) return null;
    if (known.has(clean)) return clean;
    const rel = resolvePath(path, clean);
    return known.has(rel) ? rel : null;
  };

  const docLink = (to: string, children: ReactNode, key: string) => (
    <a
      key={key}
      href={knowledgeHash(slug, to)}
      onClick={(e) => {
        e.preventDefault();
        onOpen(to);
      }}
      className="text-link hover:text-link-hover"
      title={to}
    >
      {children}
    </a>
  );

  const link: LinkRender = (href, children, key) => {
    if (/^https?:\/\//i.test(href))
      return (
        <a key={key} href={href} target="_blank" rel="noopener noreferrer nofollow" className="text-link hover:text-link-hover">
          {children}
        </a>
      );
    // Another scheme (javascript:, mailto:, file:…) or a protocol-relative URL never becomes a link; MAP's own
    // memory (`map://…`) does when it is indexed.
    const scheme = /^[a-z][a-z0-9+.-]*:/i.test(href) || href.startsWith("//");
    const to = scheme ? (known.has(href) ? href : null) : target(href);
    if (to) return docLink(to, children, key);
    return (
      <span key={key} className="underline decoration-line-3 decoration-dotted underline-offset-2" title={scheme || href.startsWith("#") ? href : `${href} · not an indexed document`}>
        {children}
      </span>
    );
  };

  const doc = data?.doc;
  const card = doc?.kind === "card";
  const overCap = card && doc.lines > settings.card_line_cap;
  const nearCap = card && !overCap && doc.lines > settings.card_line_warn;

  return (
    <Modal onClose={onClose} width={920} label={`Document ${path}`}>
      <header className="flex items-start justify-between gap-3 border-b border-line px-5 py-4">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            {doc && <Badge tone={KIND_TONE[doc.kind]}>{KIND_LABEL[doc.kind]}</Badge>}
            <h2 className="text-[17px] leading-tight font-semibold text-text">{doc?.title || path.split("/").pop()}</h2>
          </div>
          <p className="mt-1.5 font-mono text-[11px] break-all text-muted">{path}</p>
          {doc && (
            <p className="mt-1 flex flex-wrap gap-x-3 gap-y-0.5 font-mono text-[11px] text-muted">
              <span className={overCap ? "text-bad" : nearCap ? "text-warn" : undefined}>
                {doc.lines} lines{card ? ` of ${settings.card_line_cap}` : ""}
              </span>
              <span>{fmtBytes(doc.bytes)}</span>
              {doc.modified && <span>modified {ago(new Date(doc.modified).getTime(), now)}</span>}
              {card && <span className={doc.in_index ? "text-ok" : "text-warn"}>{doc.in_index ? "in the index" : "not in the index"}</span>}
            </p>
          )}
        </div>
        <button onClick={onClose} className="flex size-7 shrink-0 items-center justify-center rounded-md text-muted hover:bg-white/5 hover:text-text" aria-label="Close">
          <X className="size-4" />
        </button>
      </header>

      {doc && doc.sources.length > 0 && (
        <div className="flex flex-col gap-1.5 border-b border-line px-5 py-3">
          <span className="label">sources · the files this card summarises</span>
          <div className="flex flex-wrap gap-1.5">
            {doc.sources.map((s) => {
              const to = target(s);
              return to ? (
                <span key={s} className="rounded border border-line-2 px-1.5 py-0.5 font-mono text-[11px]">
                  {docLink(to, s, s)}
                </span>
              ) : (
                <span key={s} className="rounded border border-line px-1.5 py-0.5 font-mono text-[11px] text-text-2" title="Not an indexed document (code or another file)">
                  {s}
                </span>
              );
            })}
          </div>
        </div>
      )}

      <div className="max-h-[70vh] overflow-y-auto px-5 py-4">
        {!ours && <p className="text-sm text-muted">Loading…</p>}
        {ours && "error" in ours && <p className="rounded-lg border border-bad/40 bg-bad/10 px-3 py-2 text-sm text-bad">Could not open {path}: {ours.error}</p>}
        {data && (
          <>
            {parts.front !== null && (
              <details className="mb-4 rounded-lg border border-line bg-inset px-3 py-2">
                <summary className="cursor-pointer font-mono text-[11px] text-muted">frontmatter</summary>
                <pre className="mt-2 overflow-x-auto font-mono text-[11.5px] leading-[1.6] text-text-2">{parts.front}</pre>
              </details>
            )}
            {!parts.body.trim() ? (
              <p className="text-sm text-muted">This document is empty.</p>
            ) : isMarkdown(path) ? (
              <Markdown text={parts.body} link={link} />
            ) : (
              <pre className="overflow-x-auto font-mono text-[12px] leading-[1.6] whitespace-pre-wrap text-text-2">{parts.body}</pre>
            )}
          </>
        )}
      </div>
    </Modal>
  );
}
