//! Processor 9: profile-specific final pass.
//!
//! - **Casing commands** (coding): "camel case user profile service" →
//!   `userProfileService`. The command word consumes the following word run
//!   up to the next punctuation/symbol or the end of the segment.
//! - **Email layout**: a greeting-only segment gets a blank line after it;
//!   a sign-off-only segment is set off on its own line.
//! - **Bullets** (meeting): each final segment becomes a "- " bullet.

use od_core_types::{FormatConfig, PipelineCtx, Segment};

use crate::TextProcessor;

/// Casing transforms, longest trigger first.
const CASINGS: &[(&str, Casing)] = &[
    ("screaming snake case", Casing::ScreamingSnake),
    ("snake case", Casing::Snake),
    ("camel case", Casing::Camel),
    ("pascal case", Casing::Pascal),
    ("kebab case", Casing::Kebab),
    ("title case", Casing::Title),
    ("all caps", Casing::AllCaps),
];

#[derive(Clone, Copy)]
enum Casing {
    Camel,
    Pascal,
    Snake,
    ScreamingSnake,
    Kebab,
    Title,
    AllCaps,
}

impl Casing {
    fn join(self, words: &[&str]) -> String {
        let lower: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
        let cap = |w: &str| {
            let mut cs = w.chars();
            match cs.next() {
                Some(f) => f.to_uppercase().chain(cs).collect::<String>(),
                None => String::new(),
            }
        };
        match self {
            Casing::Camel => {
                let mut out = lower[0].clone();
                for w in &lower[1..] {
                    out.push_str(&cap(w));
                }
                out
            }
            Casing::Pascal => lower.iter().map(|w| cap(w)).collect(),
            Casing::Snake => lower.join("_"),
            Casing::ScreamingSnake => lower.join("_").to_uppercase(),
            Casing::Kebab => lower.join("-"),
            Casing::Title => {
                let capped: Vec<String> = lower.iter().map(|w| cap(w)).collect();
                capped.join(" ")
            }
            Casing::AllCaps => words.join(" ").to_uppercase(),
        }
    }
}

const GREETINGS: &[&str] = &[
    "good morning",
    "good afternoon",
    "good evening",
    "hello",
    "hey",
    "dear",
    "hi",
];

const SIGN_OFFS: &[&str] = &[
    "best regards",
    "kind regards",
    "warm regards",
    "many thanks",
    "thank you",
    "sincerely",
    "regards",
    "thanks",
    "cheers",
    "best",
];

/// Applies the profile's format pass per [`FormatConfig`].
pub struct ProfileFormat {
    cfg: FormatConfig,
}

impl ProfileFormat {
    /// Builds the processor from profile configuration.
    pub fn new(cfg: FormatConfig) -> Self {
        Self { cfg }
    }
}

/// Applies casing commands. Word run = alphanumeric tokens separated by
/// single spaces; anything else (punctuation, symbol, newline) ends the run.
fn apply_casing_commands(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    'outer: while i < text.len() {
        let at_word_start = text[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        if at_word_start {
            for (trigger, casing) in CASINGS {
                if !lower[i..].starts_with(trigger) {
                    continue;
                }
                let after = i + trigger.len();
                if lower[after..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_alphanumeric())
                {
                    continue; // trigger is a prefix of a longer word
                }
                // Collect the following word run.
                let mut words: Vec<&str> = Vec::new();
                let mut j = after;
                loop {
                    let rest = &text[j..];
                    let skipped = rest.len() - rest.trim_start_matches(' ').len();
                    let word_start = j + skipped;
                    let word_len = text[word_start..]
                        .chars()
                        .take_while(|c| c.is_alphanumeric())
                        .map(char::len_utf8)
                        .sum::<usize>();
                    if word_len == 0 {
                        break;
                    }
                    words.push(&text[word_start..word_start + word_len]);
                    j = word_start + word_len;
                }
                if words.is_empty() {
                    break; // command with nothing to case: leave literal
                }
                out.push_str(&casing.join(&words));
                i = j;
                continue 'outer;
            }
        }
        let c = text[i..].chars().next().expect("in bounds");
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// True when the whole segment is just a greeting ("Hi Sam,").
fn is_greeting(text: &str) -> bool {
    let t = text.trim().trim_end_matches(['.', ',', '!']).trim();
    let lower = t.to_ascii_lowercase();
    GREETINGS.iter().any(|g| {
        lower.starts_with(g)
            && lower[g.len()..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_alphanumeric())
            // at most a short name after the greeting word
            && t[g.len()..].split_whitespace().count() <= 3
    })
}

/// True when the whole segment is just a sign-off ("Best regards,").
fn is_sign_off(text: &str) -> bool {
    let t = text
        .trim()
        .trim_end_matches(['.', ',', '!'])
        .trim()
        .to_ascii_lowercase();
    SIGN_OFFS.contains(&t.as_str())
}

impl TextProcessor for ProfileFormat {
    fn name(&self) -> &'static str {
        "profile_format"
    }

    fn process(&self, seg: &mut Segment, _ctx: &PipelineCtx) {
        if self.cfg.casing_commands {
            seg.text = apply_casing_commands(&seg.text);
        }
        if self.cfg.email_layout && !seg.text.is_empty() {
            if is_greeting(&seg.text) {
                seg.text.push('\n');
            } else if is_sign_off(&seg.text) {
                seg.text = format!("\n{}\n", seg.text);
            }
        }
        if self.cfg.bullets && !seg.text.is_empty() && !seg.text.starts_with("- ") {
            // Trailing newline so consecutive finals form a list, not a
            // run-on line (insertion turns '\n' into Return).
            seg.text = format!("- {}\n", seg.text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::golden;

    fn with(cfg: FormatConfig) -> ProfileFormat {
        ProfileFormat::new(cfg)
    }

    fn casing() -> ProfileFormat {
        with(FormatConfig {
            casing_commands: true,
            ..FormatConfig::default()
        })
    }

    #[test]
    fn casing_golden() {
        golden(
            &casing(),
            &[
                ("camel case user profile service", "userProfileService"),
                ("snake case max retry count", "max_retry_count"),
                ("pascal case http client", "HttpClient"),
                ("kebab case main nav bar", "main-nav-bar"),
                ("screaming snake case api key", "API_KEY"),
                ("title case quarterly report", "Quarterly Report"),
                ("all caps warning label", "WARNING LABEL"),
                ("let camel case foo bar = 1", "let fooBar = 1"),
                ("camel case x, then prose", "x, then prose"),
                ("a camelcase word stays", "a camelcase word stays"),
                ("camel case", "camel case"),
                ("no commands here", "no commands here"),
            ],
        );
    }

    #[test]
    fn email_golden() {
        let proc = with(FormatConfig {
            email_layout: true,
            ..FormatConfig::default()
        });
        golden(
            &proc,
            &[
                ("Hi Sam,", "Hi Sam,\n"),
                ("Good morning team,", "Good morning team,\n"),
                ("Best regards,", "\nBest regards,\n"),
                ("Thanks.", "\nThanks.\n"),
                (
                    "Hi is a word in this long sentence body",
                    "Hi is a word in this long sentence body",
                ),
                ("The best option wins.", "The best option wins."),
            ],
        );
    }

    #[test]
    fn bullets_golden() {
        let proc = with(FormatConfig {
            bullets: true,
            ..FormatConfig::default()
        });
        golden(
            &proc,
            &[
                ("decided to ship Friday.", "- decided to ship Friday.\n"),
                ("- already a bullet", "- already a bullet"),
                ("", ""),
            ],
        );
    }
}
