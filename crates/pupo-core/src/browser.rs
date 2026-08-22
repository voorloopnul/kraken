//! What the browser pane knows that has nothing to do with drawing a page.
//!
//! Which is: what a typed address means, and what counts as an empty tab. Both
//! are small and both are wrong in ways that are hard to see from a screenshot —
//! a bare hostname that gets searched for instead of visited, a fresh tab that
//! counts as a page and so keeps the panel open — so they live here where a test
//! can hold them still.

/// The URLs a tab shows when it is showing nothing.
pub const BLANK: &str = "about:blank";

/// Whether a tab is on nothing. A fresh tab sits on `about:blank`, which costs
/// no renderer and must not be mistaken for a page worth keeping the panel open
/// for.
pub fn is_blank(url: &str) -> bool {
    url.is_empty() || url == BLANK
}

/// A typed address as a URL to load.
///
/// The rule is the one every browser uses: something that already names a scheme
/// is taken as it is, something that looks like a host is visited over https,
/// and anything else is a search. The distinction that matters is the middle
/// one — `localhost:8080` and `example.com/a b` are a host and a search, and
/// getting them the wrong way round either searches for the page you meant or
/// tries to visit the sentence you typed.
pub fn navigate_to(input: &str, search: &str) -> String {
    let text = input.trim();
    if text.is_empty() {
        return BLANK.to_string();
    }
    if text.contains(char::is_whitespace) {
        // An address has no spaces in it, whatever else it looks like.
        return format!("{search}{}", percent_encode(text));
    }
    if has_scheme(text) {
        return text.to_string();
    }
    if looks_like_host(text) {
        return format!("https://{text}");
    }
    format!("{search}{}", percent_encode(text))
}

/// Schemes that carry no `//` and are still addresses rather than sentences.
const BARE_SCHEMES: [&str; 4] = ["about:", "mailto:", "data:", "file:"];

/// Whether the text already names a scheme.
///
/// `://` rather than a bare colon, because a bare colon is far more often a port
/// or a sentence than a scheme: `localhost:8080` is a host and
/// `note:remember this` is a search, and reading either as a scheme visits
/// something nobody asked for. The handful of schemes that legitimately have no
/// `//` are listed rather than guessed at.
fn has_scheme(text: &str) -> bool {
    if BARE_SCHEMES.iter().any(|scheme| text.starts_with(scheme)) {
        return true;
    }
    let Some((head, _)) = text.split_once("://") else {
        return false;
    };
    let mut chars = head.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// Whether the text names a host rather than describing a search.
///
/// A space rules it out at once. Past that it is either a name with a dot in it
/// (`example.com`, `10.0.0.1`) or one of the names that resolve without one
/// (`localhost`), with an optional port and path after either.
fn looks_like_host(text: &str) -> bool {
    if text.contains(char::is_whitespace) {
        return false;
    }
    let authority = text
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let host = authority.split(':').next().unwrap_or_default();
    if host.is_empty() {
        return false;
    }
    // A port has to be a number; `note:remember this` is not a host on :8080.
    if let Some((_, port)) = authority.split_once(':') {
        if port.is_empty() || !port.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
    }
    if host == "localhost" {
        return true;
    }
    // A dot inside the name, not at either end: `example.com` yes, `.` and
    // `hello.` no.
    match host.find('.') {
        Some(0) => false,
        Some(at) => at + 1 < host.len() && !host.contains(".."),
        None => false,
    }
}

/// The few characters that would otherwise change what a query string means.
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEARCH: &str = "https://duckduckgo.com/?q=";

    #[test]
    fn a_scheme_is_left_alone() {
        assert_eq!(navigate_to("https://a.dev/x", SEARCH), "https://a.dev/x");
        assert_eq!(navigate_to("file:///tmp/a.html", SEARCH), "file:///tmp/a.html");
    }

    #[test]
    fn a_bare_host_is_visited_over_https() {
        assert_eq!(navigate_to("example.com", SEARCH), "https://example.com");
        assert_eq!(navigate_to("example.com/a", SEARCH), "https://example.com/a");
        assert_eq!(navigate_to("10.0.0.1", SEARCH), "https://10.0.0.1");
    }

    #[test]
    fn localhost_is_a_host_even_without_a_dot() {
        assert_eq!(navigate_to("localhost:8080", SEARCH), "https://localhost:8080");
        assert_eq!(navigate_to("localhost", SEARCH), "https://localhost");
    }

    #[test]
    fn a_sentence_is_a_search() {
        assert_eq!(
            navigate_to("how do i exit vim", SEARCH),
            "https://duckduckgo.com/?q=how+do+i+exit+vim"
        );
    }

    #[test]
    fn a_colon_that_is_not_a_port_is_a_search() {
        // The trap this rule exists for: a scheme cannot begin with a digit and
        // a port cannot contain letters, so neither of these is an address.
        assert!(navigate_to("note:remember this", SEARCH).starts_with(SEARCH));
        assert!(navigate_to("example.com:abc", SEARCH).starts_with(SEARCH));
    }

    #[test]
    fn a_lone_dot_is_not_a_host() {
        assert!(navigate_to("hello.", SEARCH).starts_with(SEARCH));
        assert!(navigate_to(".", SEARCH).starts_with(SEARCH));
    }

    #[test]
    fn a_search_is_escaped() {
        assert_eq!(
            navigate_to("a&b=c", SEARCH),
            "https://duckduckgo.com/?q=a%26b%3Dc"
        );
    }

    #[test]
    fn nothing_typed_loads_nothing() {
        assert_eq!(navigate_to("   ", SEARCH), BLANK);
    }

    #[test]
    fn a_fresh_tab_counts_as_empty() {
        assert!(is_blank(""));
        assert!(is_blank(BLANK));
        assert!(!is_blank("https://example.com"));
    }
}
