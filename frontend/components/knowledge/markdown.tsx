/** A small, safe Markdown renderer for the Knowledge doc viewer. No library is installed, and the text comes from files
 * agents write, so nothing here ever reaches the DOM as HTML: every block and inline piece is a React element with
 * the text as children. It covers what knowledge cards use: headings, paragraphs, lists (with task boxes), fenced
 * code, inline code, bold/italic, quotes, rules and pipe tables. Links are handed to `link`, which decides what may
 * become an anchor. */

import clsx from "clsx";
import { Fragment, type ReactNode } from "react";

type Item = { depth: number; marker: string; text: string };
type Block =
  | { t: "h"; level: number; text: string }
  | { t: "p"; text: string }
  | { t: "code"; lang: string; text: string }
  | { t: "list"; items: Item[] }
  | { t: "quote"; text: string }
  | { t: "hr" }
  | { t: "table"; head: string[]; rows: string[][] };

const FENCE = /^\s{0,3}(```+|~~~+)\s*([\w+-]*)/;
const HEADING = /^\s{0,3}(#{1,6})\s+(.*?)\s*#*\s*$/;
const HR = /^\s{0,3}([-*_])(\s*\1){2,}\s*$/;
const LIST = /^(\s*)([-*+]|\d{1,9}[.)])\s+(.*)$/;
const QUOTE = /^\s{0,3}>\s?(.*)$/;
const TABLE_SEP = /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/;

const cells = (line: string) =>
  line
    .trim()
    .replace(/^\|/, "")
    .replace(/\|$/, "")
    .split("|")
    .map((c) => c.trim());

/** `---` frontmatter at the very top, split from the body. */
export function splitFrontmatter(text: string): { front: string | null; body: string } {
  const m = /^---\r?\n([\s\S]*?)\r?\n---\s*(\r?\n|$)/.exec(text);
  return m ? { front: m[1], body: text.slice(m[0].length) } : { front: null, body: text };
}

function parse(text: string): Block[] {
  const lines = text.replace(/\r\n?/g, "\n").split("\n");
  const out: Block[] = [];
  let i = 0;
  const startsBlock = (l: string) => FENCE.test(l) || HEADING.test(l) || HR.test(l) || LIST.test(l) || QUOTE.test(l);
  while (i < lines.length) {
    const line = lines[i];
    if (!line.trim()) {
      i++;
      continue;
    }
    const fence = FENCE.exec(line);
    if (fence) {
      const close = fence[1];
      const body: string[] = [];
      i++;
      while (i < lines.length && !lines[i].trimStart().startsWith(close)) body.push(lines[i++]);
      i++; // the closing fence (or the end of the file)
      out.push({ t: "code", lang: fence[2], text: body.join("\n") });
      continue;
    }
    const h = HEADING.exec(line);
    if (h) {
      out.push({ t: "h", level: h[1].length, text: h[2] });
      i++;
      continue;
    }
    if (HR.test(line)) {
      out.push({ t: "hr" });
      i++;
      continue;
    }
    if (line.trimStart().startsWith("|") && i + 1 < lines.length && TABLE_SEP.test(lines[i + 1])) {
      const head = cells(line);
      const rows: string[][] = [];
      i += 2;
      while (i < lines.length && lines[i].trimStart().startsWith("|")) rows.push(cells(lines[i++]));
      out.push({ t: "table", head, rows });
      continue;
    }
    if (QUOTE.test(line)) {
      const body: string[] = [];
      while (i < lines.length && QUOTE.test(lines[i])) body.push(QUOTE.exec(lines[i++])![1]);
      out.push({ t: "quote", text: body.join(" ") });
      continue;
    }
    if (LIST.test(line)) {
      const items: Item[] = [];
      while (i < lines.length) {
        const m = LIST.exec(lines[i]);
        if (m) {
          items.push({ depth: Math.floor(m[1].replace(/\t/g, "  ").length / 2), marker: m[2], text: m[3] });
          i++;
        } else if (lines[i].trim() && /^\s+/.test(lines[i]) && items.length) {
          // An indented continuation line belongs to the item above.
          items[items.length - 1].text += ` ${lines[i].trim()}`;
          i++;
        } else break;
      }
      out.push({ t: "list", items });
      continue;
    }
    const body: string[] = [];
    while (i < lines.length && lines[i].trim() && !startsBlock(lines[i]) && !(lines[i].trimStart().startsWith("|") && TABLE_SEP.test(lines[i + 1] ?? ""))) body.push(lines[i++].trim());
    out.push({ t: "p", text: body.join(" ") });
  }
  return out;
}

export type LinkRender = (href: string, children: ReactNode, key: string) => ReactNode;

// Code spans first, so nothing inside them is formatted; `_italic_` only between non-word characters (snake_case
// names stay as they are); bare http(s) URLs become links too.
const INLINE =
  /(`+)([\s\S]+?)\1|\*\*([\s\S]+?)\*\*|(?<!\w)__([\s\S]+?)__(?!\w)|\*(?!\s)([^*]+?)\*|(?<!\w)_(?!\s)([^_]+?)_(?!\w)|(!?)\[([^\]]*)\]\(\s*<?([^)\s>]*)>?(?:\s+"[^"]*")?\s*\)|(https?:\/\/[^\s<>()]+[^\s<>().,;:!?'"])/g;

function inline(text: string, link: LinkRender, key = "i"): ReactNode[] {
  const out: ReactNode[] = [];
  let last = 0;
  let n = 0;
  for (const m of text.matchAll(INLINE)) {
    const at = m.index;
    if (at > last) out.push(text.slice(last, at));
    const k = `${key}.${n++}`;
    if (m[2] !== undefined) {
      out.push(
        <code key={k} className="rounded bg-inset px-1 py-px font-mono text-[0.9em] text-text">
          {m[2].trim()}
        </code>,
      );
    } else if (m[3] !== undefined || m[4] !== undefined) {
      out.push(
        <strong key={k} className="font-semibold text-text">
          {inline(m[3] ?? m[4], link, k)}
        </strong>,
      );
    } else if (m[5] !== undefined || m[6] !== undefined) {
      out.push(<em key={k}>{inline(m[5] ?? m[6], link, k)}</em>);
    } else if (m[8] !== undefined) {
      // An image is shown as its alt text: the viewer never loads files from the page.
      out.push(m[7] ? <span key={k} className="text-muted">[image: {m[8] || m[9]}]</span> : link(m[9], inline(m[8], link, k), k));
    } else if (m[10] !== undefined) {
      out.push(link(m[10], m[10], k));
    }
    last = at + m[0].length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}

/** A task-list box at the start of an item: `[ ]` or `[x]`. */
function itemBody(text: string, link: LinkRender, key: string): ReactNode {
  const box = /^\[([ xX])\]\s+/.exec(text);
  if (!box) return inline(text, link, key);
  return (
    <>
      <span className={clsx("mr-1.5 font-mono", box[1] === " " ? "text-muted" : "text-ok")}>{box[1] === " " ? "☐" : "☑"}</span>
      {inline(text.slice(box[0].length), link, key)}
    </>
  );
}

const HEADING_CLASS = ["", "mt-6 text-[22px] font-semibold", "mt-6 text-[18px] font-semibold", "mt-5 text-[15px] font-semibold", "mt-4 text-[13.5px] font-semibold", "mt-4 text-[13px] font-semibold text-text-2", "mt-4 text-[12.5px] font-semibold text-muted"];

export function Markdown({ text, link }: { text: string; link: LinkRender }) {
  const blocks = parse(text);
  return (
    <div className="flex flex-col gap-3 text-[13.5px] leading-[1.65] break-words text-text-2 [&>*:first-child]:mt-0">
      {blocks.map((b, i) => {
        const k = `b${i}`;
        switch (b.t) {
          case "h": {
            const Tag = `h${b.level}` as "h1";
            return (
              <Tag key={k} className={clsx("leading-tight tracking-[-0.01em] text-text", HEADING_CLASS[b.level])}>
                {inline(b.text, link, k)}
              </Tag>
            );
          }
          case "p":
            return <p key={k}>{inline(b.text, link, k)}</p>;
          case "code":
            return (
              <pre key={k} className="overflow-x-auto rounded-lg border border-line bg-inset px-3.5 py-3 font-mono text-[12px] leading-[1.6] text-text" data-lang={b.lang || undefined}>
                <code>{b.text}</code>
              </pre>
            );
          case "quote":
            return (
              <blockquote key={k} className="border-l-2 border-line-3 pl-3 text-muted">
                {inline(b.text, link, k)}
              </blockquote>
            );
          case "hr":
            return <hr key={k} className="border-line" />;
          case "list":
            return (
              <ul key={k} className="flex flex-col gap-1">
                {b.items.map((it, j) => (
                  <li key={j} className="flex gap-2" style={{ paddingLeft: it.depth * 18 }}>
                    <span className="shrink-0 font-mono text-[12px] text-muted">{/\d/.test(it.marker) ? it.marker : "•"}</span>
                    <span className="min-w-0">{itemBody(it.text, link, `${k}.${j}`)}</span>
                  </li>
                ))}
              </ul>
            );
          case "table":
            return (
              <div key={k} className="overflow-x-auto rounded-lg border border-line">
                <table className="w-full border-collapse text-left text-[12.5px]">
                  <thead>
                    <tr className="border-b border-line bg-panel-2">
                      {b.head.map((c, j) => (
                        <th key={j} className="px-3 py-1.5 font-semibold text-text">
                          {inline(c, link, `${k}.h${j}`)}
                        </th>
                      ))}
                    </tr>
                  </thead>
                  <tbody>
                    {b.rows.map((r, j) => (
                      <tr key={j} className="border-b border-line last:border-0">
                        {r.map((c, x) => (
                          <td key={x} className="px-3 py-1.5 align-top">
                            {inline(c, link, `${k}.${j}.${x}`)}
                          </td>
                        ))}
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            );
        }
      })}
    </div>
  );
}

/** `[`…`]` in a search snippet marks the matches. */
export function Snippet({ text }: { text: string }) {
  const parts = text.split(/(\[[^[\]]*\])/g);
  return (
    <>
      {parts.map((p, i) =>
        p.length > 2 && p.startsWith("[") && p.endsWith("]") ? (
          <mark key={i} className="rounded-sm bg-accent/25 px-0.5 text-text">
            {p.slice(1, -1)}
          </mark>
        ) : (
          <Fragment key={i}>{p}</Fragment>
        ),
      )}
    </>
  );
}
