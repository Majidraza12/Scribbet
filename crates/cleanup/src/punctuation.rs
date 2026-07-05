//! Processor 5: punctuation repair.
//!
//! Three jobs, each independently configurable:
//! - spoken punctuation words → marks ("comma" → ","), attached to the
//!   preceding word, with a determiner guard so "the period of time"
//!   survives;
//! - comma-splice repair around conjunctive adverbs ("I ran, however I
//!   fell" → "I ran; however, I fell");
//! - terminal punctuation ("this works" → "this works.").

use od_core_types::{PipelineCtx, PunctuationConfig, Segment};

use crate::TextProcessor;

const SPOKEN: &[(&str, &str)] = &[
    ("exclamation point", "!"),
    ("exclamation mark", "!"),
    ("question mark", "?"),
    ("full stop", "."),
    ("semicolon", ";"),
    ("period", "."),
    ("comma", ","),
];

/// When the word before a spoken punctuation name is one of these, the user
/// was talking *about* the mark (or a period of time), not dictating it.
const DETERMINER_GUARD: &[&str] = &[
    "the", "a", "an", "this", "that", "every", "each", "any", "one", "no", "first", "second",
    "last", "next",
];

/// Conjunctive adverbs that mark a comma splice when they join two clauses
/// with only a comma.
const SPLICE_ADVERBS: &[&str] = &[
    "however",
    "therefore",
    "moreover",
    "furthermore",
    "consequently",
    "nevertheless",
    "otherwise",
    "meanwhile",
];

/// Repairs STT punctuation per profile configuration.
pub struct Punctuation {
    cfg: PunctuationConfig,
}

impl Punctuation {
    /// Builds the processor from profile configuration.
    pub fn new(cfg: PunctuationConfig) -> Self {
        Self { cfg }
    }
}

/// Converts spoken punctuation words to marks attached to the previous
/// word. SPOKEN is ordered longest-first so "exclamation mark" wins over
/// any later single-word form.
fn convert_spoken(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    'outer: while i < text.len() {
        let at_word_start = text[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        if at_word_start {
            for (spoken, mark) in SPOKEN {
                if lower[i..].starts_with(spoken)
                    && lower[i + spoken.len()..]
                        .chars()
                        .next()
                        .is_none_or(|c| !c.is_alphanumeric())
                    && !guarded(&lower, i)
                {
                    // Attach: drop the space(s) before the word.
                    while out.ends_with([' ', '\t']) {
                        out.pop();
                    }
                    out.push_str(mark);
                    i += spoken.len();
                    // Swallow one following space so "a comma , b" seams
                    // don't double up; the general seam fix re-adds it.
                    if text[i..].starts_with(' ') {
                        out.push(' ');
                        i += 1;
                    }
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

/// True when the word just before byte `i` is a determiner from the guard
/// list ("the period", "a comma", ...).
fn guarded(lower: &str, i: usize) -> bool {
    let before = lower[..i].trim_end();
    let word_start = before
        .rfind(|c: char| !c.is_alphanumeric())
        .map_or(0, |p| p + 1);
    DETERMINER_GUARD.contains(&&before[word_start..])
}

/// ", however I fell" → "; however, I fell" for known conjunctive adverbs.
/// Only fires when the adverb is followed by more words (an actual second
/// clause), not at the end ("I fell, however." reads fine).
fn repair_splices(text: &str) -> String {
    let mut out = text.to_owned();
    // Positions shift after an edit, so rescan until stable (segments are
    // short; this is cheap and only runs on finals).
    'restart: loop {
        let lower = out.to_ascii_lowercase();
        for adv in SPLICE_ADVERBS {
            let needle = format!(", {adv} ");
            if let Some(pos) = lower.find(&needle) {
                out.replace_range(pos..pos + needle.len(), &format!("; {adv}, "));
                continue 'restart;
            }
        }
        return out;
    }
}

impl TextProcessor for Punctuation {
    fn name(&self) -> &'static str {
        "punctuation"
    }

    fn process(&self, seg: &mut Segment, _ctx: &PipelineCtx) {
        let mut text = std::mem::take(&mut seg.text);
        if self.cfg.spoken {
            text = convert_spoken(&text);
            if text.ends_with(' ') {
                text.truncate(text.trim_end().len());
            }
        }
        text = repair_splices(&text);
        if self.cfg.ensure_terminal {
            if let Some(last) = text.chars().next_back() {
                if last.is_alphanumeric() {
                    text.push('.');
                }
            }
        }
        seg.text = text;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::golden;

    fn default_proc() -> Punctuation {
        Punctuation::new(PunctuationConfig::default())
    }

    #[test]
    fn punctuation_golden() {
        golden(
            &default_proc(),
            &[
                ("this works comma right", "this works, right."),
                ("done period", "done."),
                ("really question mark", "really?"),
                ("stop exclamation mark", "stop!"),
                ("wait full stop", "wait."),
                ("ends without mark", "ends without mark."),
                ("already ends.", "already ends."),
                ("ends with brace {", "ends with brace {"),
                ("", ""),
                ("I ran, however I fell", "I ran; however, I fell."),
                ("I fell, however.", "I fell, however."),
                ("3.14 stays 3.14", "3.14 stays 3.14."),
                // determiner guard: talking about the mark, not dictating it
                ("the period of time", "the period of time."),
                ("add a comma here", "add a comma here."),
                ("that comma looks wrong", "that comma looks wrong."),
            ],
        );
    }

    #[test]
    fn spoken_disabled_leaves_words() {
        let proc = Punctuation::new(PunctuationConfig {
            enabled: true,
            spoken: false,
            ensure_terminal: true,
        });
        golden(&proc, &[("works comma right", "works comma right.")]);
    }

    #[test]
    fn no_terminal_when_disabled() {
        let proc = Punctuation::new(PunctuationConfig {
            enabled: true,
            spoken: true,
            ensure_terminal: false,
        });
        golden(&proc, &[("open ended", "open ended")]);
    }
}
