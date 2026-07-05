//! Processor 8: user-defined regex → replacement rules, in order.

use od_core_types::{PipelineCtx, Segment, UserRule};
use regex::Regex;

use crate::TextProcessor;

/// Applies the profile's ordered regex rules. Patterns compile once at
/// chain build; an invalid pattern is skipped with a warning (never a
/// per-segment failure).
pub struct UserRules {
    rules: Vec<(Regex, String)>,
}

impl UserRules {
    /// Compiles the profile's rules, skipping invalid patterns.
    pub fn new(rules: &[UserRule]) -> Self {
        let rules = rules
            .iter()
            .filter_map(|r| match Regex::new(&r.pattern) {
                Ok(re) => Some((re, r.replacement.clone())),
                Err(e) => {
                    tracing::warn!(pattern = %r.pattern, "invalid user rule skipped: {e}");
                    None
                }
            })
            .collect();
        Self { rules }
    }
}

impl TextProcessor for UserRules {
    fn name(&self) -> &'static str {
        "user_rules"
    }

    fn process(&self, seg: &mut Segment, _ctx: &PipelineCtx) {
        for (re, replacement) in &self.rules {
            let replaced = match re.replace_all(&seg.text, replacement.as_str()) {
                std::borrow::Cow::Owned(s) => Some(s),
                std::borrow::Cow::Borrowed(_) => None,
            };
            if let Some(s) = replaced {
                seg.text = s;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::golden;

    fn rule(pattern: &str, replacement: &str) -> UserRule {
        UserRule {
            pattern: pattern.into(),
            replacement: replacement.into(),
        }
    }

    #[test]
    fn user_rules_golden() {
        let proc = UserRules::new(&[
            rule(r"\bgonna\b", "going to"),
            rule(r"\bwanna\b", "want to"),
            rule(r"(\d+) bucks", "$$$1"),
        ]);
        golden(
            &proc,
            &[
                ("I'm gonna go", "I'm going to go"),
                ("wanna bet 50 bucks", "want to bet $50"),
                ("gonnabe stays", "gonnabe stays"),
            ],
        );
    }

    #[test]
    fn rules_apply_in_order() {
        let proc = UserRules::new(&[rule("a", "b"), rule("bb", "c")]);
        golden(&proc, &[("ab", "c")]);
    }

    #[test]
    fn invalid_pattern_is_skipped() {
        let proc = UserRules::new(&[rule("(unclosed", "x"), rule("ok", "fine")]);
        golden(&proc, &[("ok (unclosed", "fine (unclosed")]);
    }
}
