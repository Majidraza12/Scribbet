//! Shared whole-word phrase replacement used by the dictionary, symbols,
//! and punctuation processors.
//!
//! Matching is byte-offset based over an ASCII-lowercased shadow of the
//! text (ASCII lowercasing preserves byte offsets; spoken forms are ASCII).
//! A match must sit on word boundaries: the characters adjacent to it may
//! not be alphanumeric.

/// One replaceable phrase, prepared for matching.
#[derive(Clone, Debug)]
pub struct PhrasePair {
    /// Spoken form; ASCII-lowercased at build time unless `case_sensitive`.
    pub spoken: String,
    /// Written replacement, inserted verbatim.
    pub written: String,
    /// Match `spoken` exactly instead of case-insensitively.
    pub case_sensitive: bool,
}

impl PhrasePair {
    /// Prepares a case-insensitive pair (the common case).
    pub fn new(spoken: &str, written: &str) -> Self {
        Self {
            spoken: spoken.to_ascii_lowercase(),
            written: written.to_owned(),
            case_sensitive: false,
        }
    }
}

/// Sorts pairs longest-spoken-first so "exclamation mark" wins over "mark"
/// and "open angle bracket" over "open angle". Call once at build time.
pub fn sort_for_matching(pairs: &mut [PhrasePair]) {
    pairs.sort_by_key(|p| std::cmp::Reverse(p.spoken.len()));
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric()
}

/// True if `text[start..end]` sits on word boundaries.
fn on_boundary(text: &str, start: usize, end: usize) -> bool {
    let before_ok = text[..start]
        .chars()
        .next_back()
        .is_none_or(|c| !is_word(c));
    let after_ok = text[end..].chars().next().is_none_or(|c| !is_word(c));
    before_ok && after_ok
}

/// Replaces every whole-word occurrence of each pair's spoken form with its
/// written form. `pairs` must be pre-sorted with [`sort_for_matching`];
/// earlier (longer) pairs win at the same position.
pub fn replace_phrases(text: &str, pairs: &[PhrasePair]) -> String {
    if pairs.is_empty() || text.is_empty() {
        return text.to_owned();
    }
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    'outer: while i < text.len() {
        // Only attempt matches at word starts.
        if on_char_word_start(text, i) {
            for p in pairs {
                let hay = if p.case_sensitive {
                    text
                } else {
                    lower.as_str()
                };
                if hay[i..].starts_with(p.spoken.as_str())
                    && on_boundary(text, i, i + p.spoken.len())
                {
                    out.push_str(&p.written);
                    i += p.spoken.len();
                    continue 'outer;
                }
            }
        }
        let c = text[i..].chars().next().expect("in bounds");
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// True if byte `i` starts a word (previous char is not word-like).
fn on_char_word_start(text: &str, i: usize) -> bool {
    text[..i].chars().next_back().is_none_or(|c| !is_word(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(list: &[(&str, &str)]) -> Vec<PhrasePair> {
        let mut v: Vec<PhrasePair> = list.iter().map(|(s, w)| PhrasePair::new(s, w)).collect();
        sort_for_matching(&mut v);
        v
    }

    #[test]
    fn replaces_whole_words_only() {
        let p = pairs(&[("arrow", "->")]);
        assert_eq!(replace_phrases("an arrow flies", &p), "an -> flies");
        assert_eq!(replace_phrases("sparrows fly", &p), "sparrows fly");
    }

    #[test]
    fn longest_phrase_wins() {
        let p = pairs(&[("open angle", "<"), ("open angle bracket", "<")]);
        assert_eq!(replace_phrases("open angle bracket x", &p), "< x");
    }

    #[test]
    fn case_insensitive_by_default() {
        let p = pairs(&[("open dictate", "OpenDictate")]);
        assert_eq!(
            replace_phrases("Open Dictate rocks", &p),
            "OpenDictate rocks"
        );
        assert_eq!(
            replace_phrases("open dictate rocks", &p),
            "OpenDictate rocks"
        );
    }

    #[test]
    fn case_sensitive_pair_matches_exactly() {
        let mut v = vec![PhrasePair {
            spoken: "API".into(),
            written: "API".into(),
            case_sensitive: true,
        }];
        sort_for_matching(&mut v);
        assert_eq!(replace_phrases("the api and API", &v), "the api and API");
    }

    #[test]
    fn multibyte_neighbors_are_boundaries() {
        let p = pairs(&[("at sign", "@")]);
        assert_eq!(replace_phrases("é at sign é", &p), "é @ é");
    }
}
