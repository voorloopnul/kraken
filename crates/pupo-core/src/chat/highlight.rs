//! Syntax highlighting: the token palette and the syntax lookups around it.
//!
//! Shared so that a snippet looks the same wherever the app shows code — a
//! fenced block in the transcript, a file in the diff viewer — rather than each
//! surface inventing its own colours.
//!
//! The palette is keyed by scope *prefix*. syntect hands back a stack of dotted
//! scopes (`string.quoted.double.python`), and a token is resolved by dropping
//! atoms off the end until an entry matches, so one entry for `string` also
//! catches every flavour of string underneath it. That is the same shape as the
//! pygments token tree this table was ported from, where a `Token.Literal.String`
//! entry covered `Token.Literal.String.Doc` and the rest of the family.

use std::collections::HashMap;
use std::sync::Mutex;

use once_cell::sync::Lazy;
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};

/// One coloured run inside a line: `start`/`end` are byte offsets into that
/// line's text, so a caller can slice the line with them directly.
///
/// Runs never cover whitespace-only stretches. A colour applies to a tinted
/// background as well as to glyphs on some surfaces, and a run that swallowed
/// the gap between two tokens would paint it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub color: &'static str,
    pub italic: bool,
}

/// Dark leans on One Dark; light uses darkened One Light values that keep
/// contrast on the tinted code background.
///
/// Entries are scope prefixes, longest match wins. The roles are pygments' —
/// this table was ported from a palette keyed by pygments token types — but the
/// keys are the scopes Sublime grammars actually emit, which do not line up one
/// for one: what pygments called `Keyword.Constant` is `constant.language`
/// here, its `Keyword.Type` and bare `def`/`fn` are `storage.type`, and its
/// `Name.Builtin` family is `support`.
///
/// A `None` value is an entry that deliberately paints nothing, and it exists
/// because the walk resolves through prefixes: `keyword.operator` covers `+`
/// and `=`, which pygments kept out of the keyword family altogether, so
/// without an explicit stop they would inherit the keyword colour and a line of
/// arithmetic would come out as loud as a line of control flow.
type Palette = HashMap<&'static str, Option<(&'static str, bool)>>;

static TOKEN_COLORS: Lazy<HashMap<&'static str, Palette>> = Lazy::new(|| {
    let table = |keyword: &'static str,
                 keyword_constant: &'static str,
                 string: &'static str,
                 number: &'static str,
                 comment: &'static str,
                 function: &'static str,
                 class: &'static str,
                 builtin: &'static str,
                 decorator: &'static str,
                 tag: &'static str,
                 attribute: &'static str| {
        Palette::from([
            ("keyword", Some((keyword, false))),
            ("storage", Some((keyword, false))),
            ("keyword.operator", None),
            // Pygments' Operator.Word — `and`, `or`, `not`, `in`, `is` — which
            // grammars file under the operators the entry above just silenced.
            ("keyword.operator.word", Some((keyword, false))),
            ("keyword.operator.logical", Some((keyword, false))),
            ("constant.language", Some((keyword_constant, false))),
            ("string", Some((string, false))),
            ("constant.numeric", Some((number, false))),
            ("comment", Some((comment, true))),
            ("entity.name.function", Some((function, false))),
            ("variable.function", Some((function, false))),
            ("entity.name.class", Some((class, false))),
            ("entity.name.type", Some((class, false))),
            ("entity.name.struct", Some((class, false))),
            // A decorator is one thing to a reader — `@app.route` — but three
            // scopes to a grammar: the `@`, the dotted name, and the call. They
            // are all inside `meta.annotation`, so colouring the container is
            // what colours the decorator whole.
            ("meta.annotation", Some((decorator, false))),
            ("support", Some((builtin, false))),
            ("entity.name.tag", Some((tag, false))),
            ("entity.other.attribute-name", Some((attribute, false))),
        ])
    };
    HashMap::from([
        (
            "dark",
            table(
                "#c678dd", "#d19a66", "#98c379", "#d19a66", "#7d818c", "#61afef", "#e5c07b",
                "#56b6c2", "#e5c07b", "#e06c75", "#d19a66",
            ),
        ),
        (
            "light",
            table(
                "#96218f", "#8a5c00", "#3c7d3b", "#8a5c00", "#75786f", "#2a5fd3", "#9c6d00",
                "#077a92", "#9c6d00", "#a8232e", "#8a5c00",
            ),
        ),
    ])
});

/// The bundled grammars.
///
/// Loading them deserializes a compressed dump of every Sublime syntax syntect
/// ships — tens of milliseconds, and enough to be felt if it happened on the
/// first code fence of a reply — so it happens once, lazily, and every lookup
/// borrows from here.
static SYNTAXES: Lazy<SyntaxSet> = Lazy::new(SyntaxSet::load_defaults_newlines);

/// Syntax lookup is a linear scan of every bundled grammar, so both directions
/// are memoised. `None` is cached too: a fence language nothing claims is the
/// common case (`text`, `log`, a typo), and it must not pay for the scan twice.
static BY_LANGUAGE: Lazy<Mutex<HashMap<String, Option<&'static SyntaxReference>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static BY_FILENAME: Lazy<Mutex<HashMap<String, Option<&'static SyntaxReference>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// The bundled grammars, for callers that need to drive a parse themselves.
pub fn syntaxes() -> &'static SyntaxSet {
    &SYNTAXES
}

/// The syntax for a fence language (`python`, `ts`, `rs`), or `None`.
pub fn syntax_for_language(language: &str) -> Option<&'static SyntaxReference> {
    if language.is_empty() {
        return None;
    }
    cached(&BY_LANGUAGE, language, |key| {
        SYNTAXES.find_syntax_by_token(key)
    })
}

/// The syntax for a file's name, or `None` when nothing claims it.
///
/// Keyed on the whole path, since grammars match whole file names (`Makefile`,
/// `.gitignore`) as well as suffixes. The name is all that is consulted: the
/// content is not at hand — the diff viewer asks about the *old* side of a file
/// that may no longer exist — and a guess from a first line would colour code as
/// another language, which reads worse than leaving it plain.
pub fn syntax_for_filename(path: &str) -> Option<&'static SyntaxReference> {
    if path.is_empty() {
        return None;
    }
    cached(&BY_FILENAME, path, |key| {
        let name = key.rsplit(['/', '\\']).next().unwrap_or(key);
        let extension = name.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
        SYNTAXES
            .find_syntax_by_extension(name)
            .or_else(|| SYNTAXES.find_syntax_by_extension(extension))
    })
}

/// The `(colour, italic)` a scope should paint with, found by dropping atoms off
/// its end until an entry matches, or `None` when no prefix of it is styled.
pub fn scope_style(theme: &str, scope: &str) -> Option<(&'static str, bool)> {
    let table = TOKEN_COLORS.get(theme).unwrap_or(&TOKEN_COLORS["light"]);
    let mut candidate = scope;
    loop {
        if let Some(style) = table.get(candidate) {
            return *style;
        }
        {
            let dot = candidate.rfind('.')?;
            candidate = &candidate[..dot]
        }
    }
}

/// Token runs for every line of `source`, indexed from zero.
///
/// Lexing the source whole and slicing the result per line is what keeps a hunk
/// that starts inside a docstring from being coloured as if it were code — the
/// diff viewer shows a window onto a file, and a grammar handed one line at a
/// time has no idea what it is in the middle of.
///
/// `None` for `syntax` yields no runs at all rather than a run per line: the
/// caller renders the source plain, which is the honest thing to do when nothing
/// claimed the language.
pub fn line_spans(source: &str, syntax: Option<&SyntaxReference>, theme: &str) -> Vec<Vec<Span>> {
    let (Some(syntax), false) = (syntax, source.is_empty()) else {
        return Vec::new();
    };
    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut lines = Vec::new();
    for line in lines_with_endings(source) {
        // The bundled grammars are the newline-terminated variants, so the line
        // is parsed with its ending and the runs are clipped back to the text.
        let content = line.trim_end_matches('\n').trim_end_matches('\r');
        let ops = state.parse_line(line, &SYNTAXES).unwrap_or_default();
        let mut spans: Vec<Span> = Vec::new();
        let mut cursor = 0usize;
        for (index, op) in &ops {
            push_span(&mut spans, content, cursor, *index, &stack, theme);
            // A grammar that leaves the stack inconsistent is a bug in the
            // grammar, not something to stop colouring the file over.
            let _ = stack.apply(op);
            cursor = (*index).min(content.len());
        }
        push_span(&mut spans, content, cursor, content.len(), &stack, theme);
        lines.push(spans);
    }
    lines
}

/// One run of `content`, if it is worth colouring: styled, non-empty, and not
/// whitespace. Merged into the previous run when they abut and match, which
/// keeps a `<span>` per token from becoming a `<span>` per grammar transition.
fn push_span(
    spans: &mut Vec<Span>,
    content: &str,
    start: usize,
    end: usize,
    stack: &ScopeStack,
    theme: &str,
) {
    let (start, end) = (start.min(content.len()), end.min(content.len()));
    if start >= end || content[start..end].trim().is_empty() {
        return;
    }
    let Some((color, italic)) = stack_style(stack, theme) else {
        return;
    };
    if let Some(last) = spans.last_mut() {
        if last.end == start && last.color == color && last.italic == italic {
            last.end = end;
            return;
        }
    }
    spans.push(Span {
        start,
        end,
        color,
        italic,
    });
}

/// The style for a scope stack: the innermost scope that the palette knows.
/// Walking inward-out means a string inside a decorated function is a string,
/// not whatever its enclosing context happens to be styled as.
fn stack_style(stack: &ScopeStack, theme: &str) -> Option<(&'static str, bool)> {
    stack
        .as_slice()
        .iter()
        .rev()
        .find_map(|scope: &Scope| scope_style(theme, &scope.build_string()))
}

/// A memoised lookup. A poisoned cache falls through to the lookup itself
/// rather than failing: the answer is still correct, just no longer free.
fn cached(
    cache: &Lazy<Mutex<HashMap<String, Option<&'static SyntaxReference>>>>,
    key: &str,
    lookup: impl Fn(&str) -> Option<&'static SyntaxReference>,
) -> Option<&'static SyntaxReference> {
    let Ok(mut table) = cache.lock() else {
        return lookup(key);
    };
    if let Some(hit) = table.get(key) {
        return *hit;
    }
    let found = lookup(key);
    table.insert(key.to_string(), found);
    found
}

/// `source` split into lines that keep their terminator, which is what the
/// newline-terminated grammars expect to be fed.
fn lines_with_endings(source: &str) -> impl Iterator<Item = &str> {
    let mut rest = source;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let split = match rest.find('\n') {
            Some(index) => index + 1,
            None => rest.len(),
        };
        let (line, tail) = rest.split_at(split);
        rest = tail;
        Some(line)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colors(source: &str, language: &str, theme: &str) -> Vec<Vec<(String, &'static str)>> {
        let syntax = syntax_for_language(language);
        line_spans(source, syntax, theme)
            .into_iter()
            .zip(source.lines())
            .map(|(spans, line)| {
                spans
                    .into_iter()
                    .map(|span| (line[span.start..span.end].to_string(), span.color))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn probe_scopes() {
        use syntect::parsing::{ParseState, ScopeStack};
        for (lang, src) in [
            ("python", "@app.route\ndef go(x=True):\n    n = 3\n    return \"hi\"  # done\n"),
            ("python", "text = '''\nnot code\n'''\n"),
            ("rust", "fn main() { let x: u32 = 1; }\n"),
            ("html", "<a href=\"x\">hi</a>\n"),
        ] {
            let syntax = syntax_for_language(lang).unwrap();
            let mut state = ParseState::new(syntax);
            let mut stack = ScopeStack::new();
            for line in lines_with_endings(src) {
                let ops = state.parse_line(line, &SYNTAXES).unwrap();
                let mut cursor = 0;
                for (i, op) in &ops {
                    let text = &line[cursor..(*i).min(line.len())];
                    if !text.trim().is_empty() {
                        println!("{lang} {:?} -> {:?}", text, stack.as_slice().iter().map(|s| s.build_string()).collect::<Vec<_>>());
                    }
                    let _ = stack.apply(op);
                    cursor = *i;
                }
            }
        }
        for t in ["ts", "tsx", "jsx", "sh", "bash", "yaml", "yml", "json", "toml", "go", "c", "cpp", "java", "kotlin", "swift", "ruby", "php", "sql", "diff", "markdown", "md", "text", "console", "javascript", "js"] {
            println!("token {t} -> {:?}", syntax_for_language(t).map(|s| s.name.clone()));
        }
    }

    #[test]
    fn a_scope_resolves_through_its_prefixes() {
        assert_eq!(
            scope_style("dark", "string.quoted.double.python"),
            Some(("#98c379", false))
        );
        // The exact entry, and a scope no prefix of which is styled.
        assert_eq!(scope_style("dark", "comment"), Some(("#7d818c", true)));
        assert_eq!(scope_style("dark", "meta.function-call.python"), None);
    }

    #[test]
    fn the_longest_matching_prefix_wins() {
        // `keyword` is styled and `keyword.operator` deliberately is not, so a
        // `+` comes out plain even though `keyword` would otherwise claim it.
        // The longer entry is the one that decides.
        assert_eq!(scope_style("dark", "keyword"), Some(("#c678dd", false)));
        assert_eq!(scope_style("dark", "keyword.operator.arithmetic"), None);
        // One step longer again puts the colour back: `and`, `or` and `not` are
        // filed under the operators the middle entry silenced.
        assert_eq!(
            scope_style("dark", "keyword.operator.word.python"),
            Some(("#c678dd", false))
        );
    }

    #[test]
    fn the_two_themes_carry_the_ported_palette() {
        assert_eq!(scope_style("dark", "keyword"), Some(("#c678dd", false)));
        assert_eq!(scope_style("light", "keyword"), Some(("#96218f", false)));
        assert_eq!(scope_style("light", "comment.line"), Some(("#75786f", true)));
        // An unknown theme answers from the light table rather than not at all.
        assert_eq!(scope_style("sepia", "keyword"), Some(("#96218f", false)));
    }

    #[test]
    fn only_comments_are_italic() {
        for theme in ["dark", "light"] {
            assert_eq!(scope_style(theme, "comment").map(|s| s.1), Some(true));
            assert_eq!(scope_style(theme, "keyword").map(|s| s.1), Some(false));
            assert_eq!(scope_style(theme, "string").map(|s| s.1), Some(false));
        }
    }

    #[test]
    fn fence_languages_and_file_names_find_their_grammar() {
        assert_eq!(syntax_for_language("python").map(|s| &s.name), Some(&"Python".to_string()));
        assert_eq!(syntax_for_language("rs").map(|s| &s.name), Some(&"Rust".to_string()));
        assert_eq!(syntax_for_filename("/src/main.rs").map(|s| &s.name), Some(&"Rust".to_string()));
        // A whole-name match, not a suffix.
        assert!(syntax_for_filename("/repo/Makefile").is_some());
    }

    #[test]
    fn nothing_claimed_is_left_plain_rather_than_guessed() {
        assert!(syntax_for_language("brainfuck").is_none());
        assert!(syntax_for_language("").is_none());
        assert!(syntax_for_filename("/tmp/notes.zzz").is_none());
        assert!(syntax_for_filename("").is_none());
        // And an unlexed source yields no runs at all, not a run per line.
        assert!(line_spans("x = 1\n", None, "dark").is_empty());
        assert!(line_spans("", syntax_for_language("python"), "dark").is_empty());
    }

    #[test]
    fn a_lookup_is_cached_but_still_answers_the_same() {
        let first = syntax_for_language("python").map(|s| s.name.clone());
        let second = syntax_for_language("python").map(|s| s.name.clone());
        assert_eq!(first, second);
        assert!(syntax_for_language("nope").is_none());
        assert!(syntax_for_language("nope").is_none());
    }

    #[test]
    fn keywords_strings_and_comments_get_their_colors() {
        let lines = colors("def go():\n    return \"hi\"  # done\n", "python", "dark");
        assert_eq!(lines[0][0], ("def".to_string(), "#c678dd"));
        assert!(lines[0].iter().any(|(text, color)| text == "go" && *color == "#61afef"));
        assert!(lines[1].iter().any(|(text, color)| text.contains("\"hi\"") && *color == "#98c379"));
        assert!(lines[1].iter().any(|(text, color)| text.contains("# done") && *color == "#7d818c"));
    }

    #[test]
    fn a_run_never_covers_the_gap_between_two_tokens() {
        let lines = line_spans("def go():\n", syntax_for_language("python"), "dark");
        for span in &lines[0] {
            let text = &"def go():"[span.start..span.end];
            assert_eq!(text.trim(), text, "{text:?} carries whitespace");
        }
    }

    #[test]
    fn a_line_inside_a_multiline_string_is_still_a_string() {
        // The reason the whole source is lexed at once rather than a line at a
        // time: line 2 on its own reads as ordinary code, and only the opening
        // quote a line above it says otherwise.
        let source = "s = '''\nnot code at all\n'''\n";
        let lines = colors(source, "python", "dark");
        assert_eq!(lines[1].len(), 1);
        assert_eq!(lines[1][0], ("not code at all".to_string(), "#98c379"));
    }

    #[test]
    fn every_line_of_the_source_gets_a_row_of_its_own() {
        // The diff viewer indexes into the result by line number, so the rows
        // have to line up with the file's lines even when a line has no runs.
        let source = "x = 1\n\ny = 2\n";
        let spans = line_spans(source, syntax_for_language("python"), "dark");
        assert_eq!(spans.len(), 3);
        assert!(spans[1].is_empty());
    }

    #[test]
    fn a_source_without_a_trailing_newline_keeps_its_last_line() {
        let spans = line_spans("x = 1", syntax_for_language("python"), "dark");
        assert_eq!(spans.len(), 1);
    }
}
