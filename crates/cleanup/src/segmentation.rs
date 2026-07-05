//! Processor 6: sentence-boundary refinement after punctuation repair.
//!
//! Normalizes spacing around marks (no space before, exactly one after when
//! prose follows) and collapses accidental doubled marks (",," / ",." from
//! upstream edits). Number-internal punctuation ("3.14", "1,000") is left
//! alone; a 3+ dot run is treated as a deliberate ellipsis.

use od_core_types::{PipelineCtx, Segment};

use crate::TextProcessor;

/// Refines splits and seams the earlier processors may have left.
pub struct Segmentation;

fn is_mark(c: char) -> bool {
    matches!(c, ',' | '.' | ';' | ':' | '?' | '!')
}

impl TextProcessor for Segmentation {
    fn name(&self) -> &'static str {
        "segmentation"
    }

    fn process(&self, seg: &mut Segment, _ctx: &PipelineCtx) {
        let chars: Vec<char> = seg.text.chars().collect();
        let mut out = String::with_capacity(seg.text.len());
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if is_mark(c) {
                let prev = chars.get(i.wrapping_sub(1)).copied();
                let next = chars.get(i + 1).copied();
                let numeric = matches!(c, '.' | ',')
                    && prev.is_some_and(|p| p.is_ascii_digit())
                    && next.is_some_and(|n| n.is_ascii_digit());
                let ellipsis =
                    c == '.' && chars[i..].iter().take_while(|&&d| d == '.').count() >= 3;
                if ellipsis {
                    out.push_str("...");
                    i += chars[i..].iter().take_while(|&&d| d == '.').count();
                    continue;
                }
                if numeric {
                    out.push(c);
                    i += 1;
                    continue;
                }
                // No space before the mark.
                while out.ends_with(' ') {
                    out.pop();
                }
                // Collapse a doubled-mark run into its last mark ("? ." from
                // upstream edits becomes ".").
                let mut last = c;
                i += 1;
                while i < chars.len() {
                    let d = chars[i];
                    if is_mark(d) {
                        last = d;
                        i += 1;
                    } else if d == ' ' && chars.get(i + 1).copied().is_some_and(is_mark) {
                        i += 1; // space between doubled marks
                    } else {
                        break;
                    }
                }
                out.push(last);
                // Exactly one space after when prose follows directly.
                if let Some(&n) = chars.get(i) {
                    if n.is_alphanumeric() {
                        out.push(' ');
                    }
                }
                continue;
            }
            out.push(c);
            i += 1;
        }
        seg.text = out;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::golden;

    #[test]
    fn segmentation_golden() {
        golden(
            &Segmentation,
            &[
                ("hello , world", "hello, world"),
                ("done .next one", "done. next one"),
                ("what ?!", "what!"),
                ("oops ,, twice", "oops, twice"),
                ("wait . . really", "wait. really"),
                ("pi is 3.14 exactly", "pi is 3.14 exactly"),
                ("1,000 units", "1,000 units"),
                ("well... maybe", "well... maybe"),
                ("tight,fit", "tight, fit"),
                ("", ""),
            ],
        );
    }
}
