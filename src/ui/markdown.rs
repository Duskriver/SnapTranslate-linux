use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// Converts standard Markdown to Pango Markup for rendering in GTK4 widgets.
pub fn markdown_to_pango(markdown: &str) -> String {
    let mut pango = String::with_capacity(markdown.len() * 2);
    let parser = Parser::new_ext(
        markdown,
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES,
    );

    let mut table_column = 0;
    let mut link_destinations = Vec::new();
    let mut lists: Vec<Option<u64>> = Vec::new();

    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {}
                Tag::Link { dest_url, .. } => link_destinations.push(dest_url.to_string()),
                Tag::Table(_) => pango.push_str("<tt>"),
                Tag::TableHead => {
                    table_column = 0;
                    pango.push_str("<b>");
                }
                Tag::TableRow => table_column = 0,
                Tag::TableCell => {
                    if table_column > 0 {
                        pango.push_str(" │ ");
                    }
                    table_column += 1;
                }
                Tag::Heading { .. } => pango.push_str("<span size=\"larger\" weight=\"bold\">"),
                Tag::Strong => pango.push_str("<b>"),
                Tag::Emphasis => pango.push_str("<i>"),
                Tag::Strikethrough => pango.push_str("<s>"),
                Tag::BlockQuote(_) => pango.push_str("<i>"),
                Tag::CodeBlock(_) => {
                    pango.push_str("<tt>");
                }
                Tag::List(start) => {
                    if !lists.is_empty() && !pango.ends_with('\n') {
                        pango.push('\n');
                    }
                    lists.push(start);
                }
                Tag::Item => {
                    pango.push_str(&"  ".repeat(lists.len().saturating_sub(1)));
                    match lists.last_mut() {
                        Some(Some(number)) => {
                            pango.push_str(&format!("{number}. "));
                            *number = number.saturating_add(1);
                        }
                        _ => pango.push_str("• "),
                    }
                }
                _ => {}
            },
            Event::End(tag_end) => match tag_end {
                TagEnd::Link => {
                    if let Some(url) = link_destinations.pop() {
                        pango.push_str(" (<u>");
                        pango.push_str(&escape_pango(&url));
                        pango.push_str("</u>)");
                    }
                }
                TagEnd::Table => pango.push_str("</tt>\n\n"),
                TagEnd::TableHead => pango.push_str("</b>\n"),
                TagEnd::TableRow => pango.push('\n'),
                TagEnd::Paragraph => {
                    pango.push_str(if lists.is_empty() { "\n\n" } else { "\n" });
                }
                TagEnd::Heading(_) => pango.push_str("</span>\n\n"),
                TagEnd::Strong => pango.push_str("</b>"),
                TagEnd::Emphasis => pango.push_str("</i>"),
                TagEnd::Strikethrough => pango.push_str("</s>"),
                TagEnd::BlockQuote(_) => pango.push_str("</i>\n\n"),
                TagEnd::CodeBlock => {
                    pango.push_str("</tt>\n\n");
                }
                TagEnd::Item => {
                    if !pango.ends_with('\n') {
                        pango.push('\n');
                    }
                }
                TagEnd::List(_) => {
                    lists.pop();
                    if lists.is_empty() {
                        pango.push('\n');
                    }
                }
                _ => {}
            },
            Event::Text(text) => {
                let escaped = escape_pango(&text);
                pango.push_str(&escaped);
            }
            Event::Code(code) => {
                pango.push_str("<tt>");
                pango.push_str(&escape_pango(&code));
                pango.push_str("</tt>");
            }
            Event::SoftBreak => pango.push('\n'),
            Event::HardBreak => pango.push('\n'),
            // Keep literal HTML as readable text; it is never executed or interpreted.
            Event::Html(text) | Event::InlineHtml(text) => pango.push_str(&escape_pango(&text)),
            Event::Rule => pango.push_str("────────\n\n"),
            _ => {}
        }
    }

    pango.trim_end().to_string()
}

fn escape_pango(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\'' => out.push_str("&apos;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_pango_special_chars() {
        assert_eq!(
            escape_pango("a & b < c > 'd' \"e\""),
            "a &amp; b &lt; c &gt; &apos;d&apos; &quot;e&quot;"
        );
    }

    #[test]
    fn test_heading_and_bold() {
        let md = "# Title\n\nThis is **bold** text.";
        let res = markdown_to_pango(md);
        assert!(res.contains("<span size=\"larger\" weight=\"bold\">Title</span>"));
        assert!(res.contains("<b>bold</b>"));
    }

    #[test]
    fn test_inline_code_and_list() {
        let md = "Here is `code`:\n- Item 1\n- Item 2";
        let res = markdown_to_pango(md);
        assert!(res.contains("<tt>"));
        assert!(res.contains("&lt;code&gt;") || res.contains("code"));
        assert!(res.contains("• Item 1"));
    }

    fn displayed_text(markdown: &str) -> String {
        gtk4::pango::parse_markup(&markdown_to_pango(markdown), '\0')
            .expect("renderer must produce valid native markup")
            .1
            .to_string()
    }

    #[test]
    fn copied_rendered_text_has_no_markdown_syntax_and_preserves_unicode() {
        let text = displayed_text(
            "# 你好 🌍\n\n**粗体** 和 *斜体*，`a < b && c`。\n\n[链接](https://example.com)",
        );
        assert_eq!(
            text,
            "你好 🌍\n\n粗体 和 斜体，a < b && c。\n\n链接 (https://example.com)"
        );
    }

    #[test]
    fn ordered_and_nested_lists_keep_numbering_and_hierarchy() {
        let text = displayed_text("3. 第一项\n   - 子项\n4. 第二项");
        assert_eq!(text, "3. 第一项\n  • 子项\n4. 第二项");
    }

    #[test]
    fn code_and_html_are_literal_readable_text() {
        let text = displayed_text(
            "```rust\nlet text = \"**不是粗体** & <tag>\";\n```\n\n<kbd>Ctrl+C</kbd>",
        );
        assert!(text.contains("let text = \"**不是粗体** & <tag>\";"));
        assert!(text.ends_with("<kbd>Ctrl+C</kbd>"));
    }

    #[test]
    fn incomplete_stream_markdown_still_renders_without_losing_text() {
        let text = displayed_text("已收到 **部分文字\n\n```\nx < y & z");
        assert!(text.contains("已收到 **部分文字"));
        assert!(text.contains("x < y & z"));
    }
    #[test]
    fn line_breaks_and_table_cells_survive_final_formatting() {
        assert_eq!(displayed_text("第一行\n第二行"), "第一行\n第二行");
        assert_eq!(
            displayed_text("| 项目 | 数值 |\n| --- | --- |\n| A | **1** |\n| B | |"),
            "项目 │ 数值\nA │ 1\nB │ \n"
        );
    }
}
