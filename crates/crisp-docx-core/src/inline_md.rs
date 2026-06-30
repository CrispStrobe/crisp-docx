//! Minimal inline-markdown → styled-span parser for footnote bodies.
//!
//! `inject_footnotes` originally dropped each note's text into a single
//! `<w:t>` run, so `*De pace fidei*` reached Word as the literal asterisks.
//! This parser turns the "light" markdown subset that actually shows up in
//! academic note text into [`Span`]s the footnote writer can emit as proper
//! runs:
//!
//!   * `*italic*`, `**bold**`, `***bold italic***` — asterisk delimiters,
//!     toggle-matched. Underscores are deliberately left literal: `_` is far
//!     too common inside prose, identifiers and transliterations to treat as
//!     emphasis without mangling text.
//!   * `[label](url)` — explicit links.
//!   * `<https://…>` angle autolinks and bare `http(s)://…` URLs. Trailing
//!     sentence punctuation (`. , ; : ! ?` and an unbalanced `)`) is not
//!     swallowed into the URL.
//!   * `\*` / `\[` … — backslash escapes the next character.
//!
//! The grammar is intentionally forgiving rather than CommonMark-exact:
//! unbalanced delimiters degrade to literal text instead of erroring, which
//! is the right behaviour for note text of unknown provenance.

/// A run of note text with resolved styling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    /// `Some(url)` when this span is a hyperlink.
    pub link: Option<String>,
}

/// Parse `input` into consecutive styled spans (left to right).
pub(crate) fn parse_inline(input: &str) -> Vec<Span> {
    let chars: Vec<char> = input.chars().collect();
    let mut spans: Vec<Span> = Vec::new();
    let mut buf = String::new();
    let mut bold = false;
    let mut italic = false;
    let mut i = 0;

    macro_rules! flush {
        () => {
            if !buf.is_empty() {
                spans.push(Span {
                    text: std::mem::take(&mut buf),
                    bold,
                    italic,
                    link: None,
                });
            }
        };
    }

    while i < chars.len() {
        let c = chars[i];
        match c {
            // Backslash escape: the next char is always literal.
            '\\' if i + 1 < chars.len() => {
                buf.push(chars[i + 1]);
                i += 2;
            }
            '*' => {
                let mut n = 1;
                while i + n < chars.len() && chars[i + n] == '*' {
                    n += 1;
                }
                let take = n.min(3);
                flush!();
                match take {
                    1 => italic = !italic,
                    2 => bold = !bold,
                    _ => {
                        bold = !bold;
                        italic = !italic;
                    }
                }
                // Asterisks beyond a run of three are literal.
                for _ in take..n {
                    buf.push('*');
                }
                i += n;
            }
            '[' => {
                if let Some((label, url, consumed)) = parse_link(&chars, i) {
                    flush!();
                    spans.push(Span {
                        text: label,
                        bold,
                        italic,
                        link: Some(url),
                    });
                    i += consumed;
                } else {
                    buf.push('[');
                    i += 1;
                }
            }
            '<' => {
                if let Some((url, consumed)) = parse_angle_autolink(&chars, i) {
                    flush!();
                    spans.push(Span {
                        text: url.clone(),
                        bold,
                        italic,
                        link: Some(url),
                    });
                    i += consumed;
                } else {
                    buf.push('<');
                    i += 1;
                }
            }
            'h' if is_boundary(&chars, i) => {
                if let Some((url, consumed)) = parse_bare_url(&chars, i) {
                    flush!();
                    spans.push(Span {
                        text: url.clone(),
                        bold,
                        italic,
                        link: Some(url),
                    });
                    i += consumed;
                } else {
                    buf.push(c);
                    i += 1;
                }
            }
            _ => {
                buf.push(c);
                i += 1;
            }
        }
    }
    flush!();
    spans
}

/// A bare URL may only start where the preceding char isn't part of a word.
fn is_boundary(chars: &[char], i: usize) -> bool {
    i == 0 || !chars[i - 1].is_alphanumeric()
}

/// `[label](url)` starting at `chars[start] == '['`. Returns
/// `(label, url, chars_consumed)`. Brackets/parens are matched non-nested,
/// which is all the link text in practice needs.
fn parse_link(chars: &[char], start: usize) -> Option<(String, String, usize)> {
    debug_assert_eq!(chars[start], '[');
    let close_label = find(chars, start + 1, ']')?;
    if close_label + 1 >= chars.len() || chars[close_label + 1] != '(' {
        return None;
    }
    let close_url = find(chars, close_label + 2, ')')?;
    let label: String = chars[start + 1..close_label].iter().collect();
    let url: String = chars[close_label + 2..close_url].iter().collect();
    let url = url.trim().to_string();
    if url.is_empty() {
        return None;
    }
    Some((label, url, close_url + 1 - start))
}

/// `<scheme:...>` autolink starting at `chars[start] == '<'`.
fn parse_angle_autolink(chars: &[char], start: usize) -> Option<(String, usize)> {
    debug_assert_eq!(chars[start], '<');
    let close = find(chars, start + 1, '>')?;
    let inner: String = chars[start + 1..close].iter().collect();
    if inner.contains(char::is_whitespace) || !looks_like_url(&inner) {
        return None;
    }
    Some((inner, close + 1 - start))
}

/// Bare `http(s)://…` URL starting at `start`. Stops at whitespace or a
/// delimiter that can't be part of a URL, then trims trailing sentence
/// punctuation.
fn parse_bare_url(chars: &[char], start: usize) -> Option<(String, usize)> {
    const SCHEMES: [&[char]; 2] = [
        &['h', 't', 't', 'p', ':', '/', '/'],
        &['h', 't', 't', 'p', 's', ':', '/', '/'],
    ];
    let scheme_len = SCHEMES
        .iter()
        .find(|s| chars[start..].starts_with(s))
        .map(|s| s.len())?;
    let mut end = start + scheme_len;
    while end < chars.len() {
        let c = chars[end];
        if c.is_whitespace()
            || matches!(
                c,
                '<' | '>' | '"' | '|' | '\\' | '^' | '`' | '*' | '[' | ']'
            )
        {
            break;
        }
        end += 1;
    }
    // Trim trailing punctuation that is almost always sentence-level, not
    // part of the link. A closing paren is kept only if the URL opened one.
    let mut url: Vec<char> = chars[start..end].to_vec();
    while let Some(&last) = url.last() {
        let strip = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' => true,
            ')' => !url.contains(&'('),
            _ => false,
        };
        if strip {
            url.pop();
        } else {
            break;
        }
    }
    if url.len() <= scheme_len {
        return None;
    }
    let consumed = url.len();
    Some((url.into_iter().collect(), consumed))
}

fn find(chars: &[char], from: usize, target: char) -> Option<usize> {
    chars[from..]
        .iter()
        .position(|&c| c == target)
        .map(|p| from + p)
}

fn looks_like_url(s: &str) -> bool {
    s.starts_with("http://")
        || s.starts_with("https://")
        || s.starts_with("mailto:")
        || s.starts_with("ftp://")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str) -> Span {
        Span {
            text: text.into(),
            bold: false,
            italic: false,
            link: None,
        }
    }

    #[test]
    fn plain_text_is_one_span() {
        assert_eq!(parse_inline("just text"), vec![plain("just text")]);
    }

    #[test]
    fn italic_and_bold() {
        let s = parse_inline("a *b* c **d** e");
        assert_eq!(s[0], plain("a "));
        assert_eq!(
            s[1],
            Span {
                text: "b".into(),
                bold: false,
                italic: true,
                link: None
            }
        );
        assert_eq!(s[2], plain(" c "));
        assert_eq!(
            s[3],
            Span {
                text: "d".into(),
                bold: true,
                italic: false,
                link: None
            }
        );
        assert_eq!(s[4], plain(" e"));
    }

    #[test]
    fn bold_italic_triple() {
        let s = parse_inline("***x***");
        assert_eq!(
            s,
            vec![Span {
                text: "x".into(),
                bold: true,
                italic: true,
                link: None
            }]
        );
    }

    #[test]
    fn underscores_are_literal() {
        assert_eq!(
            parse_inline("universale_in_re"),
            vec![plain("universale_in_re")]
        );
    }

    #[test]
    fn escaped_asterisk() {
        assert_eq!(parse_inline(r"a \* b"), vec![plain("a * b")]);
    }

    #[test]
    fn explicit_link() {
        let s = parse_inline("see [here](https://example.org/p) now");
        assert_eq!(s[0], plain("see "));
        assert_eq!(
            s[1],
            Span {
                text: "here".into(),
                bold: false,
                italic: false,
                link: Some("https://example.org/p".into())
            }
        );
        assert_eq!(s[2], plain(" now"));
    }

    #[test]
    fn bare_url_drops_trailing_period() {
        let s = parse_inline("vgl. https://lto.de/n/2u17424.");
        assert_eq!(s[1].link.as_deref(), Some("https://lto.de/n/2u17424"));
        assert_eq!(s[2], plain("."));
    }

    #[test]
    fn intraword_h_is_not_a_url() {
        assert_eq!(parse_inline("Pythagoras"), vec![plain("Pythagoras")]);
    }

    #[test]
    fn italic_link_label() {
        let s = parse_inline("*[t](http://x.io)*");
        assert_eq!(
            s,
            vec![Span {
                text: "t".into(),
                bold: false,
                italic: true,
                link: Some("http://x.io".into())
            }]
        );
    }
}
