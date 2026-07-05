//! Processor 2: filler removal, position-aware.
//!
//! Two classes of filler:
//! - **Unconditional** ("um", "uh", ...): never content words; removed at
//!   any whole-word position.
//! - **Conditional** ("like", "you know", "i mean", ...): real words that
//!   are only fillers when set off from the clause — removed only when
//!   preceded by a comma or the segment start AND followed by a comma,
//!   terminal punctuation, or the segment end. "I like it" survives;
//!   "it was, like, huge" loses the aside.
//!
//! Removal also consumes one adjacent comma so "it was, like, huge" becomes
//! "it was huge", not "it was, huge".

use od_core_types::{FillerConfig, PipelineCtx, Segment};

use crate::TextProcessor;

const UNCONDITIONAL: &[&str] = &["um", "uh", "uhm", "erm", "er"];
const CONDITIONAL: &[&str] = &["like", "you know", "i mean", "sort of", "kind of"];

/// Removes disfluencies. Built from [`FillerConfig`]; `extra` entries are
/// treated as unconditional (the user listed them deliberately).
pub struct FillerRemoval {
    unconditional: Vec<String>,
    conditional: Vec<String>,
}

impl FillerRemoval {
    /// Builds the processor from profile configuration.
    pub fn new(cfg: &FillerConfig) -> Self {
        let mut unconditional: Vec<String> =
            UNCONDITIONAL.iter().map(|s| (*s).to_owned()).collect();
        unconditional.extend(cfg.extra.iter().map(|s| s.to_ascii_lowercase()));
        Self {
            unconditional,
            conditional: CONDITIONAL.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    fn remove_all(&self, text: &str) -> String {
        let mut current = text.to_owned();
        // Iterate until stable: removing one filler can expose another
        // ("um, uh, hello" — the second's left context becomes the start).
        loop {
            let next = self.remove_pass(&current);
            if next == current {
                return current;
            }
            current = next;
        }
    }

    fn remove_pass(&self, text: &str) -> String {
        let lower = text.to_ascii_lowercase();
        for (word, conditional) in self
            .unconditional
            .iter()
            .map(|w| (w, false))
            .chain(self.conditional.iter().map(|w| (w, true)))
        {
            let mut from = 0;
            while let Some(rel) = lower[from..].find(word.as_str()) {
                let start = from + rel;
                let end = start + word.len();
                if !on_word_boundary(&lower, start, end) {
                    from = start + 1;
                    continue;
                }
                if !conditional || aside_context(&lower, start, end) {
                    return splice_out(text, start, end);
                }
                from = end;
            }
        }
        text.to_owned()
    }
}

fn on_word_boundary(text: &str, start: usize, end: usize) -> bool {
    let before = text[..start].chars().next_back();
    let after = text[end..].chars().next();
    before.is_none_or(|c| !c.is_alphanumeric()) && after.is_none_or(|c| !c.is_alphanumeric())
}

/// True when the match is set off like an aside: left context (ignoring
/// spaces) is a comma or the segment start; right context is a comma,
/// terminal punctuation, or the end.
fn aside_context(text: &str, start: usize, end: usize) -> bool {
    let left = text[..start].trim_end();
    let left_ok = left.is_empty() || left.ends_with(',');
    let right = text[end..].trim_start();
    let right_ok = right.is_empty() || right.starts_with([',', '.', '?', '!']);
    left_ok && right_ok
}

/// Removes `text[start..end]` plus the commas that set it off, then repairs
/// the seam (no doubled spaces, no orphaned commas).
fn splice_out(text: &str, start: usize, end: usize) -> String {
    let mut left = text[..start].trim_end().to_owned();
    let mut right = text[end..].trim_start();
    let right_comma = right.starts_with(',');
    if right_comma {
        right = right[1..].trim_start();
    }
    // Drop the left comma when it bracketed the aside (a comma also followed)
    // or it would now collide with punctuation; otherwise it still separates
    // real clauses ("Yes, um no" → "Yes, no") and stays.
    if left.ends_with(',')
        && (right_comma || right.is_empty() || right.starts_with([',', '.', '?', '!', ';', ':']))
    {
        left.pop();
        let trimmed = left.trim_end().len();
        left.truncate(trimmed);
    }
    if left.is_empty() {
        return right.to_owned();
    }
    if right.is_empty() {
        return left;
    }
    if right.starts_with([',', '.', '?', '!', ';', ':']) {
        format!("{left}{right}")
    } else {
        format!("{left} {right}")
    }
}

impl TextProcessor for FillerRemoval {
    fn name(&self) -> &'static str {
        "fillers"
    }

    fn process(&self, seg: &mut Segment, _ctx: &PipelineCtx) {
        seg.text = self.remove_all(&seg.text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::golden;

    fn default_proc() -> FillerRemoval {
        FillerRemoval::new(&FillerConfig::default())
    }

    #[test]
    fn fillers_golden() {
        golden(
            &default_proc(),
            &[
                ("um hello", "hello"),
                ("Um, hello there", "hello there"),
                ("so uh I think", "so I think"),
                ("um, uh, deep breath", "deep breath"),
                ("it was, like, huge", "it was huge"),
                ("I like it", "I like it"),
                ("they like, went home", "they like, went home"),
                ("you know, it works", "it works"),
                ("tell me what you know", "tell me what you know"),
                ("i mean, seriously", "seriously"),
                ("it is, sort of, done", "it is done"),
                ("that hum is fine", "that hum is fine"),
                ("burn the umber", "burn the umber"),
                ("", ""),
            ],
        );
    }

    #[test]
    fn extra_fillers_are_unconditional() {
        let proc = FillerRemoval::new(&FillerConfig {
            enabled: true,
            extra: vec!["basically".into()],
        });
        golden(&proc, &[("basically it works basically", "it works")]);
    }

    #[test]
    fn trailing_aside_before_period() {
        golden(&default_proc(), &[("it works, you know.", "it works.")]);
    }
}
