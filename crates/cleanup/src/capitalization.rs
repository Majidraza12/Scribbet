//! Processor 7: capitalization.
//!
//! Sentence starts (segment start and after `.?!` + whitespace), standalone
//! "i" (including "i'm", "i've", ...), and profile-supplied proper nouns.
//! Coding profiles disable the whole processor — code casing is owned by
//! the profile-format casing commands.

use od_core_types::{PipelineCtx, Segment};

use crate::TextProcessor;
use crate::phrase::{PhrasePair, replace_phrases, sort_for_matching};

/// Capitalizes sentence starts, "i", and configured proper nouns.
pub struct Capitalization {
    proper: Vec<PhrasePair>,
}

impl Capitalization {
    /// `proper_nouns` come from the profile ("Majid", "Scribbet", ...);
    /// they are matched case-insensitively and written back as configured.
    pub fn new(proper_nouns: &[String]) -> Self {
        let mut proper: Vec<PhrasePair> =
            proper_nouns.iter().map(|n| PhrasePair::new(n, n)).collect();
        sort_for_matching(&mut proper);
        Self { proper }
    }
}

/// Uppercases sentence-start letters. A letter starts a sentence when only
/// whitespace or nothing precedes it, or the previous non-space character
/// is `.`, `?`, or `!` followed by whitespace (so "3.14" and "e.g.x" stay).
fn capitalize_sentences(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cap_next = true; // armed: uppercase the next letter
    let mut pending = false; // saw a terminal mark; arm on whitespace
    for c in text.chars() {
        if pending && (c.is_whitespace() || c == '\n') {
            cap_next = true;
            pending = false;
        }
        if matches!(c, '.' | '?' | '!') {
            pending = true;
            out.push(c);
            continue;
        }
        if c.is_whitespace() {
            out.push(c);
            continue;
        }
        pending = false;
        if c.is_alphabetic() && cap_next {
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
        cap_next = false;
    }
    out
}

/// "i" → "I" as a standalone word ("i", "i'm", "i've", "i'll", "i'd").
fn capitalize_standalone_i(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    for (idx, c) in text.char_indices() {
        if c == 'i' {
            let before_ok = idx == 0
                || text[..idx]
                    .chars()
                    .next_back()
                    .is_some_and(|p| !p.is_alphanumeric() && p != '\'');
            let after = bytes.get(idx + 1).copied();
            let after_ok = match after {
                None => true,
                Some(b) if !(b as char).is_ascii_alphanumeric() && b != b'\'' => true,
                _ => false,
            } || matches!(
                &bytes[idx + 1..],
                [b'\'', b'm', ..]
                    | [b'\'', b'v', b'e', ..]
                    | [b'\'', b'l', b'l', ..]
                    | [b'\'', b'd', ..]
            );
            if before_ok && after_ok {
                out.push('I');
                continue;
            }
        }
        out.push(c);
    }
    out
}

impl TextProcessor for Capitalization {
    fn name(&self) -> &'static str {
        "capitalization"
    }

    fn process(&self, seg: &mut Segment, _ctx: &PipelineCtx) {
        let mut text = capitalize_standalone_i(&seg.text);
        if !self.proper.is_empty() {
            text = replace_phrases(&text, &self.proper);
        }
        seg.text = capitalize_sentences(&text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::golden;

    fn plain() -> Capitalization {
        Capitalization::new(&[])
    }

    #[test]
    fn capitalization_golden() {
        golden(
            &plain(),
            &[
                ("hello world", "Hello world"),
                ("one. two. three", "One. Two. Three"),
                ("wait! really? yes", "Wait! Really? Yes"),
                ("i think i'm sure i'll go", "I think I'm sure I'll go"),
                ("i've done it, i'd say", "I've done it, I'd say"),
                ("pi is 3.14 not 3.15", "Pi is 3.14 not 3.15"),
                ("it is fine", "It is fine"),
                ("insider info", "Insider info"),
                ("", ""),
            ],
        );
    }

    #[test]
    fn proper_nouns_from_profile() {
        let proc = Capitalization::new(&["Majid".into(), "Scribbet".into()]);
        golden(
            &proc,
            &[(
                "tell majid that scribbet works",
                "Tell Majid that Scribbet works",
            )],
        );
    }

    #[test]
    fn unicode_sentence_start() {
        golden(&plain(), &[("état des lieux", "État des lieux")]);
    }
}
