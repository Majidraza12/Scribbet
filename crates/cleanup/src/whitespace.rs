//! Processor 1: whitespace normalization.

use od_core_types::{PipelineCtx, Segment};

use crate::TextProcessor;

/// Collapses whitespace runs, trims the ends, and normalizes unicode space
/// variants (NBSP, thin space, ...) to plain spaces. Newlines survive (a
/// later profile-format pass may have introduced none yet, but user rules
/// and email layout depend on them staying intact downstream).
pub struct Whitespace;

impl TextProcessor for Whitespace {
    fn name(&self) -> &'static str {
        "whitespace"
    }

    fn process(&self, seg: &mut Segment, _ctx: &PipelineCtx) {
        let mut out = String::with_capacity(seg.text.len());
        let mut pending_space = false;
        let mut pending_newline = false;
        for c in seg.text.chars() {
            if c == '\n' {
                pending_newline = true;
                pending_space = false;
            } else if c.is_whitespace() {
                pending_space = true;
            } else {
                if pending_newline {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                } else if pending_space && !out.is_empty() {
                    out.push(' ');
                }
                pending_newline = false;
                pending_space = false;
                out.push(c);
            }
        }
        seg.text = out;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::golden;

    #[test]
    fn whitespace_golden() {
        golden(
            &Whitespace,
            &[
                ("", ""),
                ("   ", ""),
                ("hello world", "hello world"),
                ("  hello   world  ", "hello world"),
                ("tabs\tand\u{a0}spaces", "tabs and spaces"),
                ("thin\u{2009}space", "thin space"),
                ("keep\nnewline", "keep\nnewline"),
                ("space \n around", "space\naround"),
                ("\n\nleading gone", "leading gone"),
            ],
        );
    }
}
