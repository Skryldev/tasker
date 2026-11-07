import { describe, expect, it } from "vitest";
import { render } from "@testing-library/react";
import {
  applyMarkdownAction,
  applySlashCommand,
  isSafeHref,
  markdownToPlain,
  renderMarkdown,
} from "@/lib/markdown";

describe("markdown renderer — security model", () => {
  it("never interprets HTML: script tags render as inert text", () => {
    const { container } = render(renderMarkdown('<script>alert(1)</script>', "t"));
    expect(container.querySelector("script")).toBeNull();
    expect(container.textContent).toContain("alert(1)");
  });

  it("renders only http(s)/mailto links; javascript: URLs render as text", () => {
    expect(isSafeHref("https://example.com")).toBe(true);
    expect(isSafeHref("mailto:a@b.c")).toBe(true);
    expect(isSafeHref("javascript:alert(1)")).toBe(false);
    expect(isSafeHref("data:text/html,x")).toBe(false);
    const { container } = render(renderMarkdown("[x](javascript:alert(1))", "t"));
    expect(container.querySelector("a")).toBeNull();
    expect(container.textContent).toContain("[x](javascript:alert(1))");
  });

  it("escapes img/on-attribute injection attempts as text", () => {
    const { container } = render(
      renderMarkdown('<img src=x onerror="alert(1)">', "t"),
    );
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("[onerror]")).toBeNull();
  });
});

describe("markdown renderer — supported subset", () => {
  it("renders headings, bold, italic, strike, code, links, lists, quotes", () => {
    const md = [
      "# Title",
      "## Sub",
      "**bold** and *italic* and ~~gone~~ and `code`",
      "[link](https://example.com)",
      "- one",
      "- two",
      "1. first",
      "- [x] done",
      "- [ ] todo",
      "> quoted",
      "```",
      "let x = 1;",
      "```",
    ].join("\n");
    const { container } = render(renderMarkdown(md, "t"));
    expect(container.querySelector("h4")?.textContent).toBe("Title");
    expect(container.querySelector("h5")?.textContent).toBe("Sub");
    expect(container.querySelector("strong")?.textContent).toBe("bold");
    expect(container.querySelector("em")?.textContent).toBe("italic");
    expect(container.querySelector("s")?.textContent).toBe("gone");
    expect(container.querySelector("code")?.textContent).toBe("code");
    expect(container.querySelector("a")?.getAttribute("href")).toBe("https://example.com");
    expect(container.querySelectorAll("ul").length).toBeGreaterThanOrEqual(1);
    expect(container.textContent).toContain("first");
    expect(container.querySelectorAll("input[type=checkbox]").length).toBe(2);
    expect(container.querySelector("blockquote")?.textContent).toContain("quoted");
    expect(container.querySelector("pre")?.textContent).toContain("let x = 1;");
  });

  it("renders plain text unchanged (backward compatibility)", () => {
    const { container } = render(renderMarkdown("Plain 1990s text.\nSecond line", "t"));
    expect(container.textContent).toContain("Plain 1990s text.");
    expect(container.textContent).toContain("Second line");
  });

  it("markdownToPlain strips constructs for aria-labels (whitespace normalized)", () => {
    expect(
      markdownToPlain("# Title\n**bold** and [l](https://x.y)").replace(/\s+/g, " ").trim(),
    ).toBe("Title bold and l");
  });
});

describe("applyMarkdownAction (selection math)", () => {
  it("wraps the selection for inline formats", () => {
    expect(applyMarkdownAction("ab", 0, 2, "bold")).toEqual({
      value: "**ab**",
      selStart: 2,
      selEnd: 4,
    });
    expect(applyMarkdownAction("ab", 0, 2, "strike")).toEqual({
      value: "~~ab~~",
      selStart: 2,
      selEnd: 4,
    });
  });

  it("prefixes the current line for block formats", () => {
    const r = applyMarkdownAction("line", 0, 0, "h2");
    expect(r.value).toBe("## line");
    const q = applyMarkdownAction("a\nb", 2, 3, "quote");
    expect(q.value).toBe("a\n> b");
  });

  it("renumbers numbered lists", () => {
    const r = applyMarkdownAction("1. a\n2. b", 0, 9, "numbered");
    expect(r.value).toBe("1. a\n2. b");
  });

  it("inserts links with the URL selected for typing", () => {
    const r = applyMarkdownAction("", 0, 0, "link");
    expect(r.value).toBe("[label](https://)");
    expect(r.value.slice(r.selStart, r.selEnd)).toBe("https://");
  });

  it("slash commands include the code block", () => {
    const r = applySlashCommand("", 0, 0, "codeblock");
    expect(r.value).toBe("```\n\n```");
    // Bullet at end-of-text appends on the same line boundary the editor passes.
    const b = applySlashCommand("x", 1, 1, "bullet");
    expect(b.value).toContain("- ");
  });
});
