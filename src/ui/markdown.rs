use pulldown_cmark::{Event, Parser, Tag, TagEnd};

/// Converts standard Markdown to Pango Markup for rendering in GTK4 widgets.
pub fn markdown_to_pango(markdown: &str) -> String {
    let mut pango = String::with_capacity(markdown.len() * 2);
    let parser = Parser::new(markdown);

    let mut in_code_block = false;

    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {}
                Tag::Heading { .. } => pango.push_str("<span size=\"larger\" weight=\"bold\">"),
                Tag::Strong => pango.push_str("<b>"),
                Tag::Emphasis => pango.push_str("<i>"),
                Tag::CodeBlock(_) => {
                    in_code_block = true;
                    pango.push_str("\n<tt><span background=\"#2b2b2b\" foreground=\"#e6e6e6\">");
                }
                Tag::Item => pango.push_str("  • "),
                _ => {}
            },
            Event::End(tag_end) => match tag_end {
                TagEnd::Paragraph => pango.push_str("\n\n"),
                TagEnd::Heading(_) => pango.push_str("</span>\n\n"),
                TagEnd::Strong => pango.push_str("</b>"),
                TagEnd::Emphasis => pango.push_str("</i>"),
                TagEnd::CodeBlock => {
                    in_code_block = false;
                    pango.push_str("</span></tt>\n\n");
                }
                TagEnd::Item => pango.push('\n'),
                _ => {}
            },
            Event::Text(text) => {
                let escaped = escape_pango(&text);
                pango.push_str(&escaped);
            }
            Event::Code(code) => {
                pango.push_str("<tt><span background=\"#333333\" foreground=\"#f0f0f0\">");
                pango.push_str(&escape_pango(&code));
                pango.push_str("</span></tt>");
            }
            Event::SoftBreak => {
                if in_code_block {
                    pango.push('\n');
                } else {
                    pango.push(' ');
                }
            }
            Event::HardBreak => pango.push('\n'),
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
        assert_eq!(escape_pango("a & b < c > 'd' \"e\""), "a &amp; b &lt; c &gt; &apos;d&apos; &quot;e&quot;");
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
}
