import { createMemo } from "solid-js";
import { marked } from "marked";

// Configure marked for safe rendering
marked.setOptions({
  breaks: true,       // GitHub-style line breaks
  gfm: true,          // GitHub Flavored Markdown (tables, strikethrough, etc.)
});

/**
 * Lightweight Markdown renderer for agent chat messages.
 *
 * Supports: headings, bold/italic, inline code, code blocks (with copy button),
 * tables, lists, links. Strips dangerous HTML (script, iframe, etc.).
 */
export default function MarkdownRenderer(props: { content: string }) {
  const html = createMemo(() => {
    if (!props.content) return "";
    const raw = marked.parse(props.content) as string;
    // Basic XSS sanitization: strip script/iframe/object/embed tags
    return raw
      .replace(/<script\b[^<]*(?:(?!<\/script>)<[^<]*)*<\/script>/gi, "")
      .replace(/<iframe\b[^>]*>.*?<\/iframe>/gi, "")
      .replace(/<object\b[^>]*>.*?<\/object>/gi, "")
      .replace(/<embed\b[^>]*\/?>/gi, "")
      .replace(/on\w+\s*=/gi, "data-blocked=");
  });

  return <div class="md-rendered" innerHTML={html()} />;
}
