//! Markdown → Telegram HTML converter.
//!
//! Claude outputs standard Markdown; Telegram supports a subset of HTML.
//! This module bridges the gap using pulldown-cmark for parsing and
//! a custom renderer that emits Telegram-compatible HTML.
//!
//! Supported mappings:
//!   ## Heading        → <b>Heading</b>
//!   **bold**          → <b>bold</b>
//!   *italic*          → <i>italic</i>
//!   ~~strike~~        → <s>strike</s>
//!   `code`            → <code>code</code>
//!   ```lang\nblock``` → <pre><code class="language-lang">block</code></pre>
//!   [text](url)       → <a href="url">text</a>
//!   > blockquote      → <blockquote>text</blockquote>
//!   - list item       → • list item
//!   1. ordered        → 1. item

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd, CodeBlockKind};

/// Convert Claude's Markdown to Telegram-compatible HTML.
pub fn md_to_tg_html(input: &str) -> String {
    let opts = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
    let parser = Parser::new_ext(input, opts);

    let mut out = String::with_capacity(input.len());
    let mut in_code_block = false;
    let mut list_stack: Vec<ListKind> = Vec::new();
    let mut ordered_index: u64 = 1;

    for event in parser {
        match event {
            // --- Block-level tags ---
            Event::Start(Tag::Heading { .. }) => {
                out.push_str("<b>");
            }
            Event::End(TagEnd::Heading(_)) => {
                out.push_str("</b>\n");
            }

            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => {
                if !in_code_block {
                    out.push('\n');
                }
            }

            Event::Start(Tag::BlockQuote(_)) => {
                out.push_str("<blockquote>");
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                // trim trailing newline inside blockquote
                if out.ends_with('\n') {
                    out.pop();
                }
                out.push_str("</blockquote>\n");
            }

            Event::Start(Tag::CodeBlock(kind)) => {
                in_code_block = true;
                match kind {
                    CodeBlockKind::Fenced(lang) if !lang.is_empty() => {
                        out.push_str(&format!(
                            "<pre><code class=\"language-{}\">",
                            escape_html(&lang)
                        ));
                    }
                    _ => {
                        out.push_str("<pre><code>");
                    }
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                // trim trailing newline inside code block
                if out.ends_with('\n') {
                    out.pop();
                }
                out.push_str("</code></pre>\n");
            }

            // --- Lists ---
            Event::Start(Tag::List(first)) => {
                match first {
                    Some(start) => {
                        ordered_index = start;
                        list_stack.push(ListKind::Ordered);
                    }
                    None => {
                        list_stack.push(ListKind::Unordered);
                    }
                }
            }
            Event::End(TagEnd::List(_)) => {
                list_stack.pop();
            }

            Event::Start(Tag::Item) => {
                let indent = "  ".repeat(list_stack.len().saturating_sub(1));
                match list_stack.last() {
                    Some(ListKind::Ordered) => {
                        out.push_str(&format!("{}{}. ", indent, ordered_index));
                        ordered_index += 1;
                    }
                    _ => {
                        out.push_str(&format!("{}\u{2022} ", indent));
                    }
                }
            }
            Event::End(TagEnd::Item) => {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
            }

            // --- Inline formatting ---
            Event::Start(Tag::Strong) => out.push_str("<b>"),
            Event::End(TagEnd::Strong) => out.push_str("</b>"),

            Event::Start(Tag::Emphasis) => out.push_str("<i>"),
            Event::End(TagEnd::Emphasis) => out.push_str("</i>"),

            Event::Start(Tag::Strikethrough) => out.push_str("<s>"),
            Event::End(TagEnd::Strikethrough) => out.push_str("</s>"),

            Event::Start(Tag::Link { dest_url, .. }) => {
                out.push_str(&format!("<a href=\"{}\">", escape_html(&dest_url)));
            }
            Event::End(TagEnd::Link) => {
                out.push_str("</a>");
            }

            // --- Content ---
            Event::Text(text) => {
                if in_code_block {
                    out.push_str(&escape_html(&text));
                } else {
                    out.push_str(&escape_html(&text));
                }
            }

            Event::Code(code) => {
                out.push_str(&format!("<code>{}</code>", escape_html(&code)));
            }

            Event::SoftBreak => out.push('\n'),
            Event::HardBreak => out.push('\n'),

            Event::Rule => out.push_str("\n---\n"),

            // Tables → monospace pre block (best we can do in Telegram)
            Event::Start(Tag::Table(_)) => {
                out.push_str("<pre>");
            }
            Event::End(TagEnd::Table) => {
                if out.ends_with('\n') {
                    out.pop();
                }
                out.push_str("</pre>\n");
            }
            Event::Start(Tag::TableHead) => {}
            Event::End(TagEnd::TableHead) => {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            Event::Start(Tag::TableRow) => {}
            Event::End(TagEnd::TableRow) => {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            Event::Start(Tag::TableCell) => {}
            Event::End(TagEnd::TableCell) => {
                out.push_str(" | ");
            }

            _ => {}
        }
    }

    // Clean up excessive newlines
    while out.ends_with('\n') {
        out.pop();
    }

    out
}

/// Escape HTML special characters for Telegram.
fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[derive(Clone, Copy)]
enum ListKind {
    Ordered,
    Unordered,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bold_italic() {
        assert_eq!(md_to_tg_html("**bold**"), "<b>bold</b>");
        assert_eq!(md_to_tg_html("*italic*"), "<i>italic</i>");
    }

    #[test]
    fn test_heading() {
        assert_eq!(md_to_tg_html("## Title"), "<b>Title</b>");
    }

    #[test]
    fn test_code() {
        assert_eq!(md_to_tg_html("`code`"), "<code>code</code>");
    }

    #[test]
    fn test_code_block() {
        let input = "```rust\nfn main() {}\n```";
        let expected = "<pre><code class=\"language-rust\">fn main() {}</code></pre>";
        assert_eq!(md_to_tg_html(input), expected);
    }

    #[test]
    fn test_link() {
        assert_eq!(
            md_to_tg_html("[click](https://example.com)"),
            "<a href=\"https://example.com\">click</a>"
        );
    }

    #[test]
    fn test_list() {
        let input = "- one\n- two";
        let result = md_to_tg_html(input);
        assert!(result.contains("\u{2022} one"));
        assert!(result.contains("\u{2022} two"));
    }

    #[test]
    fn test_escape_html() {
        assert_eq!(md_to_tg_html("a < b & c > d"), "a &lt; b &amp; c &gt; d");
    }

    #[test]
    fn test_plain_text_unchanged() {
        assert_eq!(md_to_tg_html("Just text"), "Just text");
    }
}
