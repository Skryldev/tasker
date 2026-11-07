import {
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import {
  Bold,
  Code,
  Heading2,
  Italic,
  Link2,
  List,
  ListOrdered,
  ListTodo,
  Quote,
  Strikethrough,
} from "lucide-react";
import { applyMarkdownAction, applySlashCommand, SLASH_COMMANDS } from "@/lib/markdown";
import { cn } from "@/lib/utils";

/**
 * Markdown editor (Phase 11): a textarea + toolbar + slash menu.
 *
 * ARCHITECTURE DECISION (documented in the report): storage is markdown; the
 * editing surface is markdown-first — a plain textarea whose selection is
 * transformed by toolbar buttons, keyboard shortcuts, and `/` slash commands
 * — instead of a contenteditable WYSIWYG. Rationale: zero new dependencies,
 * zero XSS surface (there is no rich HTML to serialize), native IME/undo, and
 * the full formatting surface still works. Read mode renders through the
 * sanitized `renderMarkdown`.
 *
 * Loaded lazily: only surfaces that enter edit mode pull this chunk.
 */

export type EditorAction =
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
  | "link";

const TOOLBAR: { action: EditorAction; label: string; icon: React.ReactNode; key?: string }[] = [
  { action: "bold", label: "Bold (Ctrl+B)", icon: <Bold />, key: "b" },
  { action: "italic", label: "Italic (Ctrl+I)", icon: <Italic />, key: "i" },
  { action: "strike", label: "Strikethrough (Ctrl+Shift+X)", icon: <Strikethrough /> },
  { action: "code", label: "Inline code", icon: <Code /> },
  { action: "h2", label: "Heading", icon: <Heading2 /> },
  { action: "bullet", label: "Bullet list", icon: <List /> },
  { action: "numbered", label: "Numbered list", icon: <ListOrdered /> },
  { action: "task", label: "Task list", icon: <ListTodo /> },
  { action: "quote", label: "Blockquote", icon: <Quote /> },
  { action: "link", label: "Link", icon: <Link2 /> },
];

export function MarkdownEditor({
  value,
  onChange,
  onSubmit,
  onCancel,
  rows = 5,
  ariaLabel,
  placeholder,
  autoFocus,
  testId,
}: {
  value: string;
  onChange: (value: string) => void;
  /** Cmd/Ctrl+Enter submits. */
  onSubmit?: () => void;
  onCancel?: () => void;
  rows?: number;
  ariaLabel: string;
  placeholder?: string;
  autoFocus?: boolean;
  testId?: string;
}) {
  const ref = useRef<HTMLTextAreaElement>(null);
  const [slashOpen, setSlashOpen] = useState(false);
  const [slashQuery, setSlashQuery] = useState("");
  const [slashIndex, setSlashIndex] = useState(0);
  const slashStartRef = useRef(0);

  const apply = (action: string, slash = false) => {
    const el = ref.current;
    if (!el) return;
    const current = el.value;
    const start = slash ? slashStartRef.current : el.selectionStart;
    const end = slash ? el.selectionStart : el.selectionEnd;
    const result =
      action === "codeblock" || slash
        ? applySlashCommand(current, start, end, action)
        : applyMarkdownAction(current, start, end, action as EditorAction);
    // For slash insertion the "/query" text must be removed first.
    const finalValue = slash
      ? result.value.slice(0, slashStartRef.current) +
        result.value.slice(el.selectionStart)
      : result.value;
    const caret = slash ? slashStartRef.current + 1 : result.selEnd;
    onChange(finalValue);
    setSlashOpen(false);
    setSlashQuery("");
    requestAnimationFrame(() => {
      el.focus();
      el.setSelectionRange(caret, slash ? caret : result.selEnd);
    });
  };

  const syncSlash = () => {
    const el = ref.current;
    if (!el) return;
    // Read from the DOM (not the prop closure): the parent state may lag a
    // keystroke behind the caret this check is about.
    const upto = el.value.slice(0, el.selectionStart);
    const lineStart = upto.lastIndexOf("\n") + 1;
    const line = upto.slice(lineStart);
    const match = /^\/([a-z]*)$/i.exec(line);
    if (match?.[1] !== undefined) {
      slashStartRef.current = lineStart;
      setSlashOpen(true);
      setSlashQuery(match[1]);
      setSlashIndex(0);
    } else {
      setSlashOpen(false);
      setSlashQuery("");
    }
  };

  const filtered = useMemo(
    () =>
      SLASH_COMMANDS.filter((c) =>
        c.label.toLowerCase().includes(slashQuery.toLowerCase()),
      ),
    [slashQuery],
  );

  const onKeyDown = (e: ReactKeyboardEvent<HTMLTextAreaElement>) => {
    if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
      e.preventDefault();
      onSubmit?.();
      return;
    }
    if (e.key === "Escape" && slashOpen) {
      e.preventDefault();
      setSlashOpen(false);
      return;
    }
    if (slashOpen && filtered.length > 0) {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setSlashIndex((i) => (i + 1) % filtered.length);
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        setSlashIndex((i) => (i - 1 + filtered.length) % filtered.length);
        return;
      }
      if (e.key === "Enter" || e.key === "Tab") {
        e.preventDefault();
        apply(String(filtered[slashIndex]?.id ?? filtered[0]?.id ?? "bullet"), true);
        return;
      }
    }
    if (!slashOpen && onCancel && e.key === "Escape") {
      e.preventDefault();
      onCancel();
      return;
    }
    // Markdown shortcuts.
    if ((e.metaKey || e.ctrlKey) && !e.altKey) {
      const key = e.key.toLowerCase();
      const map: Record<string, EditorAction> = { b: "bold", i: "italic" };
      if (e.shiftKey && key === "x") {
        e.preventDefault();
        apply("strike");
        return;
      }
      if (map[key]) {
        e.preventDefault();
        apply(map[key]);
      }
    }
  };

  return (
    <div className="relative rounded-md border border-input bg-background shadow-sm focus-within:ring-2 focus-within:ring-ring">
      {/* Toolbar: 32px, icon-only with labels for AT. */}
      <div
        className="flex h-8 items-center gap-0.5 border-b px-1"
        role="toolbar"
        aria-label="Formatting"
      >
        {TOOLBAR.map((t) => (
          <button
            key={t.action}
            type="button"
            aria-label={t.label}
            title={t.label}
            tabIndex={-1}
            onMouseDown={(e) => {
              e.preventDefault();
              apply(t.action);
            }}
            className="inline-flex h-7 w-7 items-center justify-center rounded text-muted-foreground transition-colors duration-100 hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          >
            {t.icon}
          </button>
        ))}
      </div>
      <div className="relative">
        <textarea
          ref={ref}
          value={value}
          onChange={(e) => {
            onChange(e.target.value);
            requestAnimationFrame(syncSlash);
          }}
          onKeyDown={onKeyDown}
          onClick={syncSlash}
          onKeyUp={syncSlash}
          rows={rows}
          aria-label={ariaLabel}
          aria-multiline="true"
          placeholder={placeholder}
          autoFocus={autoFocus}
          data-testid={testId}
          className="block w-full resize-y bg-transparent px-3 py-2 text-sm leading-relaxed outline-none placeholder:text-muted-foreground"
        />
        {slashOpen && filtered.length > 0 ? (
          <ul
            role="listbox"
            aria-label="Insert markdown block"
            className="absolute left-2 top-2 z-10 max-h-48 w-56 overflow-y-auto rounded-md border bg-popover p-1 shadow-md"
            data-testid="slash-menu"
          >
            {filtered.map((c, i) => (
              <li key={c.id}>
                <button
                  type="button"
                  role="option"
                  aria-selected={i === slashIndex}
                  onMouseDown={(e) => {
                    e.preventDefault();
                    apply(String(c.id), true);
                  }}
                  className={cn(
                    "flex w-full items-center rounded px-2 py-1.5 text-left text-sm transition-colors duration-100",
                    i === slashIndex ? "bg-accent text-accent-foreground" : "hover:bg-accent/60",
                  )}
                >
                  /{c.label}
                </button>
              </li>
            ))}
          </ul>
        ) : null}
      </div>
      <p className="border-t px-2 py-1 text-2xs text-muted-foreground">
        Markdown supported · / for blocks · Ctrl+Enter to save · Esc to cancel
      </p>
    </div>
  );
}
