//! Processor 3: user vocabulary and jargon replacement.

use od_core_types::{DictEntry, PipelineCtx, Segment};

use crate::TextProcessor;
use crate::phrase::{PhrasePair, replace_phrases, sort_for_matching};

/// Replaces spoken forms with the user's written forms ("open dictate" →
/// "Scribbet"). Entries come from the profile snapshot (SQLite-backed,
/// resolved by od-storage); matching is whole-word, case-insensitive unless
/// an entry opts into case sensitivity, longest spoken form first.
pub struct Dictionary {
    pairs: Vec<PhrasePair>,
}

impl Dictionary {
    /// Prepares the resolved entries for matching.
    pub fn new(entries: &[DictEntry]) -> Self {
        let mut pairs: Vec<PhrasePair> = entries
            .iter()
            .map(|e| PhrasePair {
                spoken: if e.case_sensitive {
                    e.spoken.clone()
                } else {
                    e.spoken.to_ascii_lowercase()
                },
                written: e.written.clone(),
                case_sensitive: e.case_sensitive,
            })
            .collect();
        sort_for_matching(&mut pairs);
        Self { pairs }
    }
}

impl TextProcessor for Dictionary {
    fn name(&self) -> &'static str {
        "dictionary"
    }

    fn process(&self, seg: &mut Segment, _ctx: &PipelineCtx) {
        seg.text = replace_phrases(&seg.text, &self.pairs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::golden;

    fn entry(spoken: &str, written: &str) -> DictEntry {
        DictEntry {
            spoken: spoken.into(),
            written: written.into(),
            case_sensitive: false,
        }
    }

    #[test]
    fn dictionary_golden() {
        let proc = Dictionary::new(&[
            entry("open dictate", "Scribbet"),
            entry("eye gore", "Igor"),
            entry("kubernetes", "Kubernetes"),
        ]);
        golden(
            &proc,
            &[
                ("open dictate ships", "Scribbet ships"),
                ("ask eye gore about kubernetes", "ask Igor about Kubernetes"),
                ("the gore scene", "the gore scene"),
                ("openly dictate terms", "openly dictate terms"),
            ],
        );
    }

    #[test]
    fn longest_spoken_form_wins() {
        let proc = Dictionary::new(&[entry("post", "POST"), entry("post gres", "Postgres")]);
        golden(&proc, &[("use post gres", "use Postgres")]);
    }

    #[test]
    fn case_sensitive_entry() {
        let proc = Dictionary::new(&[DictEntry {
            spoken: "Jason".into(),
            written: "JSON".into(),
            case_sensitive: true,
        }]);
        golden(&proc, &[("Jason parses jason", "JSON parses jason")]);
    }
}
