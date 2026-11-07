/**
 * Minimal, dependency-free Markdown renderer (Phase 11).
 *
 * Supported subset: headings (#..###), bold **x**, italic *x*, strikethrough
 * ~~x~~, inline code `x`, fenced code blocks ```, links [t](u), bullet /
 * numbered lists, task lists (- [ ] / - [x]), blockquotes >, paragraphs.
 *
 * SECURITY MODEL: there is no HTML passthrough. The input is rendered by
 * building React elements; every text run is emitted as a TEXT NODE, so
 * `<script>`, `on*` attributes and `javascript:` URLs cannot survive — links
 * only render for http(s)/mailto targets, everything else renders as plain
 * text. This is the sanitizer: not a filter over hostile HTML, but a
 * renderer that never interprets HTML in the first place.
 */

import type { ReactNode } from "react";

const HREF_RE = /^(https?:\/\/|mailto:)/i;

function safeHref(raw: string): string | null {
  const href = raw.trim();
  return HREF_RE.test(href) ? href : null;
}

/** Inline formatting: code spans first (verbatim), then emphasis/strikes/links. */
function renderInline(text: string, keyPrefix: string): ReactNode[] {
  const nodes: ReactNode[] = [];
  // Split on inline code first so formatting inside code is literal.
  const codeParts = text.split(/(`[^`]+`)/g);
  codeParts.forEach((part, pi) => {
    const codeMatch = /^`([^`]+)`$/.exec(part);
    if (codeMatch) {
      nodes.push(
        <code
          key={`${keyPrefix}-c${pi}`}
          className="rounded bg-muted px-1 py-0.5 font-mono text-[0.85em]"
        >
          {codeMatch[1]}
        </code>,
      );
      return;
    }
    // Links [label](url) — inside the non-code segment.
    const linkParts = part.split(/(\[[^\]]+\]\([^)\s]+\))/g);
    linkParts.forEach((lpart, li) => {
      const linkMatch = /^\[([^\]]+)\]\(([^)\s]+)\)$/.exec(lpart);
      if (linkMatch) {
        const href = safeHref(linkMatch[2] ?? "");
        if (href) {
          nodes.push(
            <a
              key={`${keyPrefix}-l${pi}-${li}`}
              href={href}
              target="_blank"
              rel="noreferrer noopener"
              className="text-primary underline underline-offset-2 hover:text-primary/80"
            >
              {linkMatch[1]}
            </a>,
          );
        } else {
          nodes.push(lpart);
        }
        return;
      }
      // Emphasis: bold, italic, strike (nested one level via sequential split).
      const emParts = lpart.split(/(\*\*[^*]+\*\*|~~[^~]+~~|\*[^*\s][^*]*\*)/g);
      emParts.forEach((epart, ei) => {
        if (/^\*\*[^*]+\*\*$/.test(epart)) {
          nodes.push(
            <strong key={`${keyPrefix}-b${pi}-${li}-${ei}`}>{epart.slice(2, -2)}</strong>,
          );
        } else if (/^~~[^~]+~~$/.test(epart)) {
          nodes.push(
            <s key={`${keyPrefix}-s${pi}-${li}-${ei}`}>{epart.slice(2, -2)}</s>,
          );
        } else if (/^\*[^*\s][^*]*\*$/.test(epart)) {
          nodes.push(<em key={`${keyPrefix}-i${pi}-${li}-${ei}`}>{epart.slice(1, -1)}</em>);
        } else if (epart) {
          nodes.push(epart);
        }
      });
    });
  });
  return nodes;
}

/** Sanitized-for-sure URL check exposed for tests. */
export function isSafeHref(raw: string): boolean {
  return safeHref(raw) !== null;
}

/**
 * Render markdown to React nodes. Every line is classified; no raw HTML
 * ever reaches the DOM (React escapes all text by construction).
 */
export function renderMarkdown(markdown: string, keyPrefix = "md"): ReactNode[] {
  const lines = markdown.replace(/\r\n/g, "\n").split("\n");
  const blocks: ReactNode[] = [];
  let i = 0;
  let key = 0;

  while (i < lines.length) {
    const line = lines[i] ?? "";

    // Fenced code block.
    if (line.trimStart().startsWith("```")) {
      const code: string[] = [];
      i += 1;
      while (i < lines.length && !(lines[i] ?? "").trimStart().startsWith("```")) {
        code.push(lines[i] ?? "");
        i += 1;
      }
      i += 1; // closing fence (or EOF)
      blocks.push(
        <pre
          key={`${keyPrefix}-${key++}`}
          className="my-2 overflow-x-auto rounded-md bg-muted p-3 font-mono text-xs leading-relaxed"
        >
          <code>{code.join("\n")}</code>
        </pre>,
      );
      continue;
    }

    // Heading.
    const heading = /^(#{1,3})\s+(.*)$/.exec(line);
    if (heading && heading[1] && heading[2] !== undefined) {
      const level = heading[1].length;
      const cls =
        level === 1
          ? "mt-3 mb-1 text-base font-semibold"
          : level === 2
            ? "mt-3 mb-1 text-sm font-semibold"
            : "mt-2 mb-1 text-sm font-semibold";
      const Tag = (`h${level + 3}` as unknown) as "h4";
      blocks.push(
        <Tag key={`${keyPrefix}-${key++}`} className={cls}>
          {renderInline(heading[2], `${keyPrefix}-${key}`)}
        </Tag>,
      );
      i += 1;
      continue;
    }

    // Blockquote.
    if (line.trimStart().startsWith(">")) {
      const quote: string[] = [];
      while (i < lines.length && (lines[i] ?? "").trimStart().startsWith(">")) {
        quote.push((lines[i] ?? "").replace(/^\s*>\s?/, ""));
        i += 1;
      }
      blocks.push(
        <blockquote
          key={`${keyPrefix}-${key++}`}
          className="my-2 border-l-2 border-border pl-3 text-muted-foreground"
        >
          {renderInline(quote.join(" "), `${keyPrefix}-${key}`)}
        </blockquote>,
      );
      continue;
    }

    // Lists (bullet, numbered, task).
    const bullet = /^(\s*)[-*]\s+(.*)$/.exec(line);
    const numbered = /^(\s*)\d+[.)]\s+(.*)$/.exec(line);
    if (bullet || numbered) {
      const isTask = (t: string) => /^(\[ \]|\[x\])\s+/.test(t);
      type Entry = { text: string; checked: boolean | null };
      const entries: Entry[] = [];
      while (i < lines.length) {
        const m = /^(\s*)([-*]|\d+[.)])\s+(.*)$/.exec(lines[i] ?? "");
        if (!m || !m[3]) break;
        let text = m[3];
        let checked: boolean | null = null;
        if (isTask(text)) {
          checked = text.startsWith("[x]");
          text = text.replace(/^(\[ \]|\[x\])\s+/, "");
        }
        entries.push({ text, checked });
        i += 1;
      }
      const hasTasks = entries.some((e) => e.checked !== null);
      const ListTag = numbered && !hasTasks ? "ol" : "ul";
      const listCls = numbered && !hasTasks ? "ml-5 list-decimal" : "ml-5 list-disc";
      blocks.push(
        <ListTag key={`${keyPrefix}-${key++}`} className={`my-1 ${listCls} space-y-0.5`}>
          {entries.map((entry, ei) =>
            entry.checked === null ? (
              <li key={ei}>{renderInline(entry.text, `${keyPrefix}-${key}-${ei}`)}</li>
            ) : (
              <li key={ei} className="list-none -ml-5 flex items-start gap-1.5">
                <input
                  type="checkbox"
                  checked={entry.checked}
                  readOnly
                  tabIndex={-1}
                  className="mt-1 h-3 w-3 accent-primary"
                />
                <span className={entry.checked ? "text-muted-foreground line-through" : ""}>
                  {renderInline(entry.text, `${keyPrefix}-${key}-${ei}`)}
                </span>
              </li>
            ),
          )}
        </ListTag>,
      );
      continue;
    }

    // Blank line.
    if (!line.trim()) {
      i += 1;
      continue;
    }

    // Paragraph (consecutive non-special lines).
    const para: string[] = [];
    while (
      i < lines.length &&
      (lines[i] ?? "").trim() &&
      !(lines[i] ?? "").trimStart().startsWith("```") &&
      !/^(#{1,3})\s+/.test(lines[i] ?? "") &&
      !(lines[i] ?? "").trimStart().startsWith(">") &&
      !/^(\s*)([-*]|\d+[.)])\s+/.test(lines[i] ?? "")
    ) {
      para.push(lines[i] ?? "");
      i += 1;
    }
    blocks.push(
      <p key={`${keyPrefix}-${key++}`} className="my-1 whitespace-pre-wrap">
        {renderInline(para.join("\n"), `${keyPrefix}-${key}`)}
      </p>,
    );
  }

  return blocks;
}

/** Strip every markdown construct — used for search indexing and aria-labels. */
export function markdownToPlain(markdown: string): string {
  return markdown
    .replace(/```[\s\S]*?```/g, " ")
    .replace(/`([^`]+)`/g, "$1")
    .replace(/\[([^\]]+)\]\([^)\s]+\)/g, "$1")
    .replace(/(\*\*|~~)/g, "")
    .replace(/\*([^*\s][^*]*)\*/g, "$1")
    .replace(/^#{1,3}\s+/gm, "")
    .replace(/^>\s?/gm, "")
    .replace(/^(\s*)([-*]|\d+[.)])\s+(\[x\])\s*/gm, "$1- [x] ")
    .replace(/^(\s*)([-*]|\d+[.)])\s+(\[ \])\s*/gm, "$1- [ ] ")
    .replace(/^(\s*)([-*]|\d+[.)])\s+/gm, "")
    .trim();
}

/**
 * Toolbar intent → wraps/inserts markdown around the current selection.
 * Pure string math (unit-tested); the editor applies the result to the
 * textarea.
 */
export function applyMarkdownAction(
  value: string,
  selStart: number,
  selEnd: number,
  action:
    | "bold"
    | "italic"
    | "strike"
    | "code"
    | "h1"
    | "h2"
    | "h3"
    | "bullet"
    | "numbered"
    | "task"
    | "quote"
    | "link",
): { value: string; selStart: number; selEnd: number } {
  const selected = value.slice(selStart, selEnd);
  const before = value.slice(0, selStart);
  const after = value.slice(selEnd);
  const lineStart = before.lastIndexOf("\n") + 1;
  const linePrefix = before.slice(lineStart);

  const wrap = (marker: string) => {
    const inner = selected || "text";
    const next = `${before}${marker}${inner}${marker}${after}`;
    return {
      value: next,
      selStart: selStart + marker.length,
      selEnd: selStart + marker.length + inner.length,
    };
  };
  const prefixLine = (prefix: string) => {
    const next = `${before.slice(0, lineStart)}${prefix}${linePrefix}${selected}${after}`;
    return {
      value: next,
      selStart: selStart + prefix.length,
      selEnd: selEnd + prefix.length,
    };
  };

  switch (action) {
    case "bold":
      return wrap("**");
    case "italic":
      return wrap("*");
    case "strike":
      return wrap("~~");
    case "code":
      return wrap("`");
    case "h1":
      return prefixLine("# ");
    case "h2":
      return prefixLine("## ");
    case "h3":
      return prefixLine("### ");
    case "bullet":
      return prefixLine("- ");
    case "task":
      return prefixLine("- [ ] ");
    case "quote":
      return prefixLine("> ");
    case "numbered": {
      const linesIn = (linePrefix + selected).split("\n");
      const renumbered = linesIn
        .map((l, idx) => `${idx + 1}. ${l.replace(/^\d+[.)]\s+/, "")}`)
        .join("\n");
      return {
        value: `${before.slice(0, lineStart)}${renumbered}${after}`,
        selStart: selStart + 3,
        selEnd: selEnd + 3,
      };
    }
    case "link": {
      const inner = selected || "label";
      const snippet = `[${inner}](https://)`;
      return {
        value: `${before}${snippet}${after}`,
        selStart: selStart + inner.length + 3,
        selEnd: selStart + snippet.length - 1,
      };
    }
    default:
      return { value, selStart, selEnd };
  }
}

/** Slash-command catalogue shared by the editor menu (exported for tests). */
export const SLASH_COMMANDS: { id: string; label: string; action: Parameters<
  typeof applyMarkdownAction
>[3] | "codeblock" }[] = [
  { id: "h1", label: "Heading 1", action: "h1" },
  { id: "h2", label: "Heading 2", action: "h2" },
  { id: "h3", label: "Heading 3", action: "h3" },
  { id: "bullet", label: "Bullet list", action: "bullet" },
  { id: "numbered", label: "Numbered list", action: "numbered" },
  { id: "task", label: "Task list", action: "task" },
  { id: "quote", label: "Blockquote", action: "quote" },
  { id: "code", label: "Inline code", action: "code" },
  { id: "codeblock", label: "Code block", action: "codeblock" },
];

/** Apply a slash command; `codeblock` needs multi-line insertion. */
export function applySlashCommand(
  value: string,
  selStart: number,
  selEnd: number,
  id: string,
): { value: string; selStart: number; selEnd: number } {
  if (id !== "codeblock") {
    const cmd = SLASH_COMMANDS.find((c) => c.id === id);
    if (cmd && cmd.action !== "codeblock") {
      return applyMarkdownAction(value, selStart, selEnd, cmd.action);
    }
  }
  const block = "```\n\n```";
  return {
    value: `${value.slice(0, selStart)}${block}${value.slice(selEnd)}`,
    selStart: selStart + 4,
    selEnd: selStart + 4,
  };
}
