//! Markdown into the rich text QML understands.
//!
//! QML's `Text` with `textFormat: Text.RichText` reads a subset of HTML 4 with
//! a little inline CSS — not a browser, and not Qt's full document engine
//! either. So this renders to *that* subset deliberately rather than to general
//! HTML: block tags, `<b>`/`<i>`, `<font>`-free inline `style` attributes on
//! spans and tables, and nothing that depends on a stylesheet the pane cannot
//! attach.
//!
//! Everything it renders is model output, which is to say untrusted text, so
//! every character that reaches the page goes through [`escape`] first. A reply
//! that says `<script>` is a reply about the word `<script>`.
//!
//! Code fences are also reported back to the caller as [`CodeBlock`]s: the pane
//! lays a card and a Copy button over each one, and hunting for `<pre>` in the
//! rendered string to find out where they are would be reading tea leaves.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::chat::highlight::{line_spans, syntax_for_language};
use crate::chat::{color, MONO_STACK};

/// A fenced code block found while rendering, in the order they appear.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeBlock {
    /// The fence's language, or empty when it carried none.
    pub language: String,
    /// The block's source, exactly as the pane should put it on the clipboard —
    /// unescaped, and without the trailing newline the fence adds.
    pub source: String,
}

/// A rendered reply: the markup, and where its code blocks were.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Rendered {
    pub html: String,
    pub code_blocks: Vec<CodeBlock>,
}

/// The five characters that would otherwise be read as markup. `'` is escaped
/// too because these strings are also handed to QML as attribute values.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Headings are sized against the reader's base size rather than from a fixed
/// scale: a reader who asked for a bigger conversation asked for bigger
/// headings in it too. The steps are Qt's own `h1`–`h6` ratios.
fn heading_size(base: i32, level: HeadingLevel) -> i32 {
    let scale = match level {
        HeadingLevel::H1 => 1.6,
        HeadingLevel::H2 => 1.4,
        HeadingLevel::H3 => 1.2,
        HeadingLevel::H4 => 1.1,
        HeadingLevel::H5 => 1.0,
        HeadingLevel::H6 => 0.9,
    };
    ((f64::from(base) * scale).round() as i32).max(1)
}

/// Render one code fence as coloured spans inside a `<pre>`.
///
/// The card behind it and the Copy button on it are the pane's business; what
/// comes back here is the text and its colours. A language nothing claims is
/// rendered plain rather than guessed at — a wrong guess colours code as another
/// language, which reads worse than leaving it alone.
fn render_code(source: &str, language: &str, theme: &str) -> String {
    let syntax = syntax_for_language(language);
    let spans = line_spans(source, syntax, theme);
    let mut out = String::new();
    out.push_str(&format!(
        "<pre style=\"font-family:{MONO_STACK}; color:{};\">",
        color(theme, "text")
    ));
    for (index, line) in source.lines().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        match spans.get(index) {
            Some(runs) if !runs.is_empty() => {
                let mut cursor = 0usize;
                for run in runs {
                    if run.start > cursor {
                        out.push_str(&escape(&line[cursor..run.start]));
                    }
                    let italic = if run.italic { "font-style:italic;" } else { "" };
                    out.push_str(&format!(
                        "<span style=\"color:{};{italic}\">{}</span>",
                        run.color,
                        escape(&line[run.start..run.end])
                    ));
                    cursor = run.end;
                }
                if cursor < line.len() {
                    out.push_str(&escape(&line[cursor..]));
                }
            }
            _ => out.push_str(&escape(line)),
        }
    }
    out.push_str("</pre>");
    out
}

/// Markdown to rich text, at `base` pixels in `theme`.
pub fn render(markdown: &str, theme: &str, base: i32) -> Rendered {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);

    let mut out = String::new();
    let mut code_blocks: Vec<CodeBlock> = Vec::new();
    // Set while inside a fence: its language, and the source accumulating.
    let mut fence: Option<(String, String)> = None;
    // Ordered lists have to number themselves; QML's rich text renders `<ol>`
    // but not its `start` attribute, so a list that begins at 3 would restart
    // at 1 without this.
    let mut list_counters: Vec<Option<u64>> = Vec::new();

    for event in Parser::new_ext(markdown, options) {
        match event {
            Event::Start(Tag::Paragraph) => out.push_str("<p>"),
            Event::End(TagEnd::Paragraph) => out.push_str("</p>"),

            Event::Start(Tag::Heading { level, .. }) => out.push_str(&format!(
                "<p style=\"font-size:{}px; font-weight:600;\">",
                heading_size(base, level)
            )),
            Event::End(TagEnd::Heading(_)) => out.push_str("</p>"),

            Event::Start(Tag::BlockQuote(_)) => out.push_str(&format!(
                "<blockquote style=\"color:{};\">",
                color(theme, "dim")
            )),
            Event::End(TagEnd::BlockQuote(_)) => out.push_str("</blockquote>"),

            Event::Start(Tag::List(start)) => {
                list_counters.push(start);
                out.push_str(if start.is_some() { "<ol>" } else { "<ul>" });
            }
            Event::End(TagEnd::List(ordered)) => {
                list_counters.pop();
                out.push_str(if ordered { "</ol>" } else { "</ul>" });
            }
            Event::Start(Tag::Item) => {
                match list_counters.last_mut() {
                    // An ordered list numbers itself, because the renderer
                    // ignores `<ol start>`.
                    Some(Some(number)) => {
                        out.push_str(&format!("<li value=\"{number}\">"));
                        *number += 1;
                    }
                    _ => out.push_str("<li>"),
                }
            }
            Event::End(TagEnd::Item) => out.push_str("</li>"),

            Event::Start(Tag::Emphasis) => out.push_str("<i>"),
            Event::End(TagEnd::Emphasis) => out.push_str("</i>"),
            Event::Start(Tag::Strong) => out.push_str("<b>"),
            Event::End(TagEnd::Strong) => out.push_str("</b>"),
            Event::Start(Tag::Strikethrough) => out.push_str("<s>"),
            Event::End(TagEnd::Strikethrough) => out.push_str("</s>"),

            Event::Start(Tag::Link { dest_url, .. }) => {
                out.push_str(&format!(
                    "<a href=\"{}\" style=\"color:{}; text-decoration:none;\">",
                    escape(&dest_url),
                    color(theme, "link")
                ));
            }
            Event::End(TagEnd::Link) => out.push_str("</a>"),

            // An image the pane cannot fetch: its alt text is the honest thing
            // to show, and a broken-image glyph is not.
            Event::Start(Tag::Image { title, .. }) => {
                out.push_str(&format!("<i>{}</i>", escape(&title)));
            }
            Event::End(TagEnd::Image) => {}

            Event::Start(Tag::Table(_)) => out.push_str(&format!(
                "<table border=\"1\" cellpadding=\"4\" cellspacing=\"0\" \
                 style=\"border-color:{};\">",
                color(theme, "code_border")
            )),
            Event::End(TagEnd::Table) => out.push_str("</table>"),
            Event::Start(Tag::TableHead) => out.push_str("<tr>"),
            Event::End(TagEnd::TableHead) => out.push_str("</tr>"),
            Event::Start(Tag::TableRow) => out.push_str("<tr>"),
            Event::End(TagEnd::TableRow) => out.push_str("</tr>"),
            Event::Start(Tag::TableCell) => out.push_str("<td>"),
            Event::End(TagEnd::TableCell) => out.push_str("</td>"),

            Event::Start(Tag::CodeBlock(kind)) => {
                let language = match kind {
                    CodeBlockKind::Fenced(info) => {
                        // ```rust,ignore and ```python title=x are both common;
                        // the language is the first word.
                        info.split([' ', ','])
                            .next()
                            .unwrap_or_default()
                            .trim()
                            .to_string()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                fence = Some((language, String::new()));
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((language, source)) = fence.take() {
                    let source = source.strip_suffix('\n').unwrap_or(&source).to_string();
                    out.push_str(&render_code(&source, &language, theme));
                    code_blocks.push(CodeBlock { language, source });
                }
            }

            Event::Text(text) => match fence.as_mut() {
                Some((_, source)) => source.push_str(&text),
                None => out.push_str(&escape(&text)),
            },
            Event::Code(text) => out.push_str(&format!(
                "<code style=\"font-family:{MONO_STACK}; background-color:{}; color:{};\">\
                 {}</code>",
                color(theme, "inline_bg"),
                color(theme, "inline_text"),
                escape(&text)
            )),
            Event::SoftBreak => match fence.as_mut() {
                Some((_, source)) => source.push('\n'),
                None => out.push(' '),
            },
            Event::HardBreak => out.push_str("<br/>"),
            // Emitted bare, and split back out by the pane: the renderer draws
            // a rule from the widget palette and ignores any colour asked for
            // here, which on a dark theme comes out as a bright bar.
            Event::Rule => out.push_str("<hr/>"),
            // Raw HTML in model output is text about HTML, not HTML: rendering
            // it would let a reply reach into the pane's own markup.
            Event::Html(text) | Event::InlineHtml(text) => out.push_str(&escape(&text)),
            Event::FootnoteReference(name) => out.push_str(&escape(&name)),
            Event::Start(Tag::FootnoteDefinition(_)) => out.push_str("<p>"),
            Event::End(TagEnd::FootnoteDefinition) => out.push_str("</p>"),
            Event::Start(Tag::HtmlBlock) | Event::End(TagEnd::HtmlBlock) => {}
            Event::Start(Tag::MetadataBlock(_)) | Event::End(TagEnd::MetadataBlock(_)) => {}
            Event::Start(Tag::DefinitionList)
            | Event::End(TagEnd::DefinitionList)
            | Event::Start(Tag::DefinitionListTitle)
            | Event::End(TagEnd::DefinitionListTitle)
            | Event::Start(Tag::DefinitionListDefinition)
            | Event::End(TagEnd::DefinitionListDefinition) => {}
            Event::Start(Tag::Superscript) | Event::End(TagEnd::Superscript) => {}
            Event::Start(Tag::Subscript) | Event::End(TagEnd::Subscript) => {}
            Event::TaskListMarker(done) => {
                out.push_str(if done { "☑ " } else { "☐ " });
            }
            // Math is off in the options above, so these never arrive; if they
            // ever do, the source is the honest thing to show.
            Event::InlineMath(text) | Event::DisplayMath(text) => {
                out.push_str(&escape(&text))
            }
        }
    }

    Rendered {
        html: out,
        code_blocks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn html(markdown: &str) -> String {
        render(markdown, "dark", 13).html
    }

    #[test]
    fn the_five_markup_characters_never_reach_the_page_as_markup() {
        assert_eq!(escape("a & b"), "a &amp; b");
        assert_eq!(escape("<b>"), "&lt;b&gt;");
        assert_eq!(escape("\"'"), "&quot;&#39;");
        assert_eq!(escape("plain"), "plain");
    }

    #[test]
    fn a_reply_about_a_script_tag_is_text_not_a_script_tag() {
        // Both as prose and as raw HTML, which pulldown reports separately.
        let rendered = html("a <script>alert(1)</script> b\n\n<div>raw</div>\n");
        assert!(!rendered.contains("<script"), "{rendered}");
        assert!(!rendered.contains("<div>"), "{rendered}");
        assert!(rendered.contains("&lt;script&gt;"));
    }

    #[test]
    fn paragraphs_and_emphasis_render_as_the_tags_qml_reads() {
        assert_eq!(
            html("plain **bold** and *italic* and ~~gone~~"),
            "<p>plain <b>bold</b> and <i>italic</i> and <s>gone</s></p>"
        );
    }

    #[test]
    fn headings_are_sized_from_the_readers_own_base() {
        let small = render("# Title", "dark", 13).html;
        let large = render("# Title", "dark", 20).html;
        assert!(small.contains("font-size:21px"), "{small}");
        assert!(large.contains("font-size:32px"), "{large}");
        // Every level is a heading, and they step down.
        let all = render("# a\n\n## b\n\n###### c", "dark", 13).html;
        assert!(all.contains("font-size:21px"));
        assert!(all.contains("font-size:18px"));
        assert!(all.contains("font-size:12px"));
    }

    #[test]
    fn a_numbered_list_keeps_the_number_it_started_at() {
        // QML's rich text ignores `<ol start>`, so the items carry their own.
        let rendered = html("3. three\n4. four\n");
        assert!(rendered.contains("<li value=\"3\">three</li>"), "{rendered}");
        assert!(rendered.contains("<li value=\"4\">four</li>"), "{rendered}");
    }

    #[test]
    fn an_unordered_list_needs_no_numbering() {
        let rendered = html("- one\n- two\n");
        assert!(rendered.starts_with("<ul><li>"), "{rendered}");
        assert!(!rendered.contains("value="));
    }

    #[test]
    fn a_link_is_tinted_and_keeps_its_target() {
        let rendered = html("see [the docs](https://example.com/a?b=1&c=2)");
        assert!(rendered.contains("href=\"https://example.com/a?b=1&amp;c=2\""), "{rendered}");
        assert!(rendered.contains(color("dark", "link")));
    }

    #[test]
    fn inline_code_is_mono_and_tinted_rather_than_body_text() {
        let rendered = html("call `render()` first");
        assert!(rendered.contains("<code style="), "{rendered}");
        assert!(rendered.contains(MONO_STACK));
        assert!(rendered.contains(color("dark", "inline_text")));
        // Its content is escaped like everything else.
        assert!(html("`<T>`").contains("&lt;T&gt;"));
    }

    #[test]
    fn a_fence_is_highlighted_and_reported_for_its_card() {
        let rendered = render("```python\ndef go():\n    return 1\n```", "dark", 13);
        assert!(rendered.html.contains("<pre style="), "{}", rendered.html);
        // `def` is a keyword, so it comes out in the keyword colour.
        assert!(rendered.html.contains("#c678dd"), "{}", rendered.html);
        assert_eq!(rendered.code_blocks.len(), 1);
        assert_eq!(rendered.code_blocks[0].language, "python");
        // The source is what the Copy button puts on the clipboard: unescaped,
        // and without the trailing newline the fence adds.
        assert_eq!(rendered.code_blocks[0].source, "def go():\n    return 1");
    }

    #[test]
    fn a_fence_in_a_language_nothing_claims_is_rendered_plain() {
        let rendered = render("```nosuchlang\nx <- 1\n```", "dark", 13);
        assert!(rendered.html.contains("x &lt;- 1"), "{}", rendered.html);
        assert!(!rendered.html.contains("<span style=\"color:"));
        assert_eq!(rendered.code_blocks[0].language, "nosuchlang");
    }

    #[test]
    fn a_fence_info_string_yields_just_the_language() {
        for info in ["rust,ignore", "rust title=main.rs", "rust"] {
            let rendered = render(&format!("```{info}\nfn f() {{}}\n```"), "dark", 13);
            assert_eq!(rendered.code_blocks[0].language, "rust", "for {info:?}");
        }
    }

    #[test]
    fn several_fences_are_reported_in_the_order_they_appear() {
        let rendered = render(
            "```sh\nls\n```\n\ntext\n\n```sh\npwd\n```",
            "dark",
            13,
        );
        assert_eq!(
            rendered
                .code_blocks
                .iter()
                .map(|b| b.source.as_str())
                .collect::<Vec<_>>(),
            ["ls", "pwd"]
        );
    }

    #[test]
    fn a_soft_break_joins_a_wrapped_paragraph_rather_than_splitting_it() {
        // Markdown treats a single newline as a space; the model wraps its
        // prose, and rendering each wrapped line as its own row would double
        // every paragraph's height.
        assert_eq!(html("one\ntwo"), "<p>one two</p>");
        assert_eq!(html("one  \ntwo"), "<p>one<br/>two</p>");
    }

    #[test]
    fn a_fence_keeps_the_line_breaks_a_paragraph_would_have_lost() {
        let rendered = render("```\na\nb\n```", "dark", 13);
        assert_eq!(rendered.code_blocks[0].source, "a\nb");
    }

    #[test]
    fn a_blockquote_and_a_rule_render_in_the_theme() {
        assert!(html("> quoted").contains(color("dark", "dim")));
        assert!(html("---").contains("<hr/>"));
    }

    #[test]
    fn a_table_renders_with_the_themes_border() {
        let rendered = html("| a | b |\n| - | - |\n| 1 | 2 |\n");
        assert!(rendered.contains("<table border=\"1\""), "{rendered}");
        assert!(rendered.contains(color("dark", "code_border")));
        assert!(rendered.contains("<td>1</td>"));
    }

    #[test]
    fn a_task_list_shows_its_boxes() {
        let rendered = html("- [x] done\n- [ ] todo\n");
        assert!(rendered.contains("☑ done"), "{rendered}");
        assert!(rendered.contains("☐ todo"), "{rendered}");
    }

    #[test]
    fn the_two_themes_render_the_same_markdown_in_their_own_colours() {
        let dark = render("`x` [a](b)", "dark", 13).html;
        let light = render("`x` [a](b)", "light", 13).html;
        assert_ne!(dark, light);
        assert!(dark.contains(color("dark", "link")));
        assert!(light.contains(color("light", "link")));
    }

    #[test]
    fn an_empty_reply_renders_to_nothing() {
        let rendered = render("", "dark", 13);
        assert!(rendered.html.is_empty());
        assert!(rendered.code_blocks.is_empty());
    }
}
