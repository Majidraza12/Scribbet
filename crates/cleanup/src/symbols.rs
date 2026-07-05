//! Processor 4: spoken symbol names → symbols.

use od_core_types::{PipelineCtx, Segment};

use crate::TextProcessor;
use crate::phrase::{PhrasePair, replace_phrases, sort_for_matching};

/// The always-safe symbol names: unambiguous multi-word phrases (plus a few
/// single words that are essentially never dictated literally).
const GENERAL: &[(&str, &str)] = &[
    ("at sign", "@"),
    ("ampersand", "&"),
    ("percent sign", "%"),
    ("dollar sign", "$"),
    ("hash sign", "#"),
    ("hashtag", "#"),
    ("underscore", "_"),
    ("asterisk", "*"),
    ("plus sign", "+"),
    ("equals sign", "="),
    ("forward slash", "/"),
    ("backslash", "\\"),
    ("degree sign", "°"),
    ("euro sign", "€"),
    ("pound sign", "£"),
    ("copyright sign", "©"),
    ("trademark sign", "™"),
    ("ellipsis", "..."),
];

/// Additions for code dictation. Includes short names ("pipe", "arrow",
/// "backtick") that would be too aggressive in prose but are what people
/// actually say while coding — which is why this is a separate table.
const PROGRAMMING: &[(&str, &str)] = &[
    ("open brace", "{"),
    ("close brace", "}"),
    ("open bracket", "["),
    ("close bracket", "]"),
    ("open paren", "("),
    ("close paren", ")"),
    ("open angle bracket", "<"),
    ("close angle bracket", ">"),
    ("arrow", "->"),
    ("fat arrow", "=>"),
    ("double colon", "::"),
    ("double equals", "=="),
    ("not equals", "!="),
    ("double ampersand", "&&"),
    ("double pipe", "||"),
    ("pipe", "|"),
    ("backtick", "`"),
    ("tilde", "~"),
    ("caret", "^"),
];

/// Replaces spoken symbol names using a built-in table selected by the
/// profile ("general" or "programming"; programming is a superset).
/// Unknown table names fall back to "general" with a warning.
pub struct Symbols {
    pairs: Vec<PhrasePair>,
}

impl Symbols {
    /// Resolves a table name from profile configuration.
    pub fn from_table(table: &str) -> Self {
        let entries: Vec<&(&str, &str)> = match table {
            "general" => GENERAL.iter().collect(),
            "programming" => GENERAL.iter().chain(PROGRAMMING).collect(),
            other => {
                tracing::warn!("unknown symbol table {other:?}; using \"general\"");
                GENERAL.iter().collect()
            }
        };
        let mut pairs: Vec<PhrasePair> = entries
            .into_iter()
            .map(|(s, w)| PhrasePair::new(s, w))
            .collect();
        sort_for_matching(&mut pairs);
        Self { pairs }
    }
}

impl TextProcessor for Symbols {
    fn name(&self) -> &'static str {
        "symbols"
    }

    fn process(&self, seg: &mut Segment, _ctx: &PipelineCtx) {
        seg.text = replace_phrases(&seg.text, &self.pairs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::golden;

    #[test]
    fn general_golden() {
        let proc = Symbols::from_table("general");
        golden(
            &proc,
            &[
                ("fifty percent sign off", "fifty % off"),
                ("email me at sign work", "email me @ work"),
                ("a hashtag win", "a # win"),
                // programming-only names must NOT fire in general prose
                ("shot an arrow", "shot an arrow"),
                ("the pipe burst", "the pipe burst"),
            ],
        );
    }

    #[test]
    fn programming_golden() {
        let proc = Symbols::from_table("programming");
        golden(
            &proc,
            &[
                ("open brace close brace", "{ }"),
                ("x arrow y", "x -> y"),
                ("a double pipe b", "a || b"),
                ("std double colon vec", "std :: vec"),
                ("value fat arrow result", "value => result"),
            ],
        );
    }

    #[test]
    fn unknown_table_falls_back_to_general() {
        let proc = Symbols::from_table("nope");
        golden(&proc, &[("open brace", "open brace")]);
    }
}
