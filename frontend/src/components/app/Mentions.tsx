import { useMemo, useState, type KeyboardEvent } from "react";
import { useMembers } from "@/api/hooks";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { applySlashCommand, renderMarkdown } from "@/lib/markdown";

/**
 * Renders comment bodies with team-member mentions highlighted. A mention is
 * `@` + a member's full email (the same token format the composer inserts and
 * the backend parses). Emails that do not belong to the team render as plain
 * text — no link affordance for strangers.
 */
export function CommentBody({ body, teamId }: { body: string; teamId: string }) {
  // Phase 11: markdown rendering (sanitized by construction — see
  // lib/markdown.tsx). Mention resolution wraps the markdown output: known
  // @email tokens are replaced with resolved display names in text form
  // before rendering, so they survive markdown parsing as plain text.
  const members = useMembers(teamId);
  const resolved = useMemo(() => {
    const known = new Map(
      (members.data?.items ?? []).map((m) => [m.user.email.toLowerCase(), m.user.display_name]),
    );
    return body.replace(
      /@([A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+)/g,
      (m, email) => { const n = known.get(String(email).toLowerCase()); return n ? `@${n}` : m; }
    );
  }, [body, members.data]);
  return <div className="text-sm leading-relaxed">{renderMarkdown(resolved, "cm")}</div>;
}

export function CommentBodyLegacy({ body, teamId }: { body: string; teamId: string }) {
  const members = useMembers(teamId);
  const parts = useMemo<Array<{ text: string; mention?: boolean }>>(() => {
    const known = new Map(
      (members.data?.items ?? []).map((m) => [m.user.email.toLowerCase(), m.user.display_name]),
    );
    const result: Array<{ text: string; mention?: boolean }> = [];
    const pattern = /@([^\s@,!;:?)\]"']+@[^\s@,!;:?)\]"']+)/g;
    let last = 0;
    for (const match of body.matchAll(pattern)) {
      const idx = match.index !== undefined ? match.index : 0;
      if (idx > last) result.push({ text: body.slice(last, idx) });
      const email = (match[1] ?? "").toLowerCase();
      result.push({ text: `@${known.get(email) ?? email}`, mention: known.has(email) });
      last = idx + match[0].length;
    }
    if (last < body.length) result.push({ text: body.slice(last) });
    return result;
  }, [body, members.data]);

  return (
    <p className="mt-1.5 whitespace-pre-wrap text-sm" data-testid="comment-body">
      {parts.map((part, i) =>
        part.mention ? (
          <span key={i} className="rounded bg-primary/10 px-0.5 font-medium text-primary">
            {part.text}
          </span>
        ) : (
          <span key={i}>{part.text}</span>
        ),
      )}
    </p>
  );
}

/**
 * Comment composer with `@` mention autocomplete over team members. Typing
 * `@` opens the member list; continuing to type filters by name or email;
 * Enter/click inserts the member's email token that the backend resolves.
 */
export function CommentComposer({
  teamId,
  value,
  onChange,
  onSubmit,
  pending,
  testIdInput = "comment-input",
  testIdSubmit = "comment-submit",
}: {
  teamId: string;
  value: string;
  onChange: (v: string) => void;
  onSubmit: () => void;
  pending?: boolean;
  testIdInput?: string;
  testIdSubmit?: string;
}) {
  const members = useMembers(teamId);
  const [mentionQuery, setMentionQuery] = useState<string | null>(null);
  const [highlight, setHighlight] = useState(0);

  const allMembers = members.data?.items ?? [];
  const matches =
    mentionQuery === null
      ? []
      : allMembers
          .filter(
            (m) =>
              m.user.display_name.toLowerCase().includes(mentionQuery.toLowerCase()) ||
              m.user.email.toLowerCase().includes(mentionQuery.toLowerCase()),
          )
          .slice(0, 5);

  const applyMention = (email: string) => {
    const at = value.lastIndexOf(`@${mentionQuery ?? ""}`);
    if (at >= 0) {
      onChange(`${value.slice(0, at)}@${email} `);
    } else {
      onChange(`${value}@${email} `);
    }
    setMentionQuery(null);
    setHighlight(0);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (mentionQuery !== null && matches.length > 0) {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setHighlight((h) => (h + 1) % matches.length);
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        setHighlight((h) => (h - 1 + matches.length) % matches.length);
        return;
      }
      if (e.key === "Enter" || e.key === "Tab") {
        e.preventDefault();
        applyMention(matches[highlight]?.user.email ?? matches[0]?.user.email ?? "");
        return;
      }
      if (e.key === "Escape") {
        setMentionQuery(null);
        return;
      }
    }
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      onSubmit();
    }
  };

  return (
    <div className="relative">
      <textarea
        value={value}
        onChange={(e) => {
          onChange(e.target.value);
          const match = /@([\w.@-]*)$/.exec(e.target.value.slice(0, e.target.selectionStart));
          setMentionQuery(match ? (match[1] ?? null) : null);
          setHighlight(0);
        }}
        onKeyDown={onKeyDown}
        placeholder="Leave a comment… use @ to mention a teammate"
        rows={2}
        className="w-full rounded-md border border-input bg-background px-2.5 py-1.5 text-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
        data-testid={testIdInput}
        aria-label="Comment"
      />
      {matches.length > 0 ? (
        <ul
          className="absolute bottom-full left-0 z-10 mb-1 w-64 overflow-hidden rounded-md border bg-popover shadow-md"
          role="listbox"
          aria-label="Mention a team member"
          data-testid="mention-list"
        >
          {matches.map((m, i) => (
            <li key={m.user.id}>
              <button
                type="button"
                role="option"
                aria-selected={i === highlight}
                onMouseDown={(e) => {
                  // Prevent the textarea losing focus before insertion.
                  e.preventDefault();
                  applyMention(m.user.email);
                }}
                className={cn(
                  "flex w-full flex-col items-start px-2.5 py-1.5 text-left text-sm",
                  i === highlight ? "bg-accent" : "",
                )}
              >
                {m.user.display_name}
                <span className="text-2xs text-muted-foreground">{m.user.email}</span>
              </button>
            </li>
          ))}
        </ul>
      ) : null}
      <div className="mt-2 flex justify-start">
        <Button size="sm" onClick={onSubmit} disabled={!value.trim() || pending} data-testid={testIdSubmit}>
          {pending ? "Sending…" : "Comment"}
        </Button>
      </div>
    </div>
  );
}
