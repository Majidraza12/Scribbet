//! Local-agreement stabilization for emulated streaming.
//!
//! Whisper hypotheses flicker at the tail (the newest words are re-heard as
//! more audio arrives). The classic fix ("local agreement", as in
//! whisper_streaming): treat the longest common prefix of two *consecutive*
//! hypotheses as stable. The stable prefix only grows, so the overlay never
//! shows text that later retracts — the tail beyond it may still change.

/// Tracks consecutive hypotheses and computes the stable prefix length.
#[derive(Debug, Default)]
pub struct LocalAgreement {
    previous: Option<String>,
    stable_len: usize,
}

impl LocalAgreement {
    /// Creates a tracker with no history.
    pub fn new() -> Self {
        Self::default()
    }

    /// Pushes the newest full-utterance hypothesis; returns the stable
    /// prefix length in bytes.
    ///
    /// Guarantees: the returned length is a char boundary of `hypothesis`,
    /// never exceeds `hypothesis.len()`, and never shrinks except when a
    /// shorter hypothesis forces it (stability is a heuristic, not a lie —
    /// if the model retracts below the old stable point, the new, shorter
    /// prefix is reported rather than pretending).
    pub fn push(&mut self, hypothesis: &str) -> usize {
        let agreed = match &self.previous {
            None => 0,
            Some(prev) => common_prefix_len(prev, hypothesis),
        };
        // Snap back to a word boundary so a half-agreed word ("hel" of
        // "hello"/"help") is not presented as stable.
        let agreed = snap_to_word_boundary(hypothesis, agreed);

        self.stable_len = self.stable_len.max(agreed).min(hypothesis.len());
        // Re-snap in case the clamp above landed mid-word or mid-char.
        self.stable_len = snap_to_word_boundary(hypothesis, self.stable_len);

        self.previous = Some(hypothesis.to_owned());
        self.stable_len
    }

    /// Clears history for a new utterance.
    pub fn reset(&mut self) {
        self.previous = None;
        self.stable_len = 0;
    }
}

/// Byte length of the longest common prefix, on a char boundary.
fn common_prefix_len(a: &str, b: &str) -> usize {
    let mut len = 0;
    for (ca, cb) in a.chars().zip(b.chars()) {
        if ca != cb {
            break;
        }
        len += ca.len_utf8();
    }
    len
}

/// Rounds `len` down to the end of the last *complete* word within
/// `text[..len]` (trailing whitespace after a word counts as complete).
fn snap_to_word_boundary(text: &str, mut len: usize) -> usize {
    while !text.is_char_boundary(len) {
        len -= 1;
    }
    let prefix = &text[..len];
    // If the cut lands exactly at the end of the text, or on whitespace, the
    // last word is complete.
    if len == text.len() || text[len..].starts_with(char::is_whitespace) {
        return len;
    }
    // Otherwise drop the partial word.
    match prefix.rfind(char::is_whitespace) {
        Some(ws) => ws + text[ws..].chars().next().map_or(1, char::len_utf8),
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_hypothesis_is_unstable() {
        let mut la = LocalAgreement::new();
        assert_eq!(la.push("hello world"), 0);
    }

    #[test]
    fn agreement_stabilizes_common_prefix() {
        let mut la = LocalAgreement::new();
        la.push("hello wor");
        let stable = la.push("hello world how");
        assert_eq!(&"hello world how"[..stable], "hello ");
    }

    #[test]
    fn stable_prefix_grows_across_pushes() {
        let mut la = LocalAgreement::new();
        la.push("the quick");
        let s1 = la.push("the quick brown");
        let s2 = la.push("the quick brown fox");
        let s3 = la.push("the quick brown fox jumps");
        assert!(s1 <= s2 && s2 <= s3);
        // The agreed region ends at the complete word "fox", so it is stable;
        // only the never-before-seen "jumps" remains unstable.
        assert_eq!(&"the quick brown fox jumps"[..s3], "the quick brown fox");
    }

    #[test]
    fn partial_word_agreement_is_not_stable() {
        let mut la = LocalAgreement::new();
        la.push("turn on the hel");
        let stable = la.push("turn on the help menu");
        // "hel" agrees byte-wise but is not a complete word.
        assert_eq!(&"turn on the help menu"[..stable], "turn on the ");
    }

    #[test]
    fn disagreement_at_start_keeps_zero() {
        let mut la = LocalAgreement::new();
        la.push("alpha beta");
        assert_eq!(la.push("gamma delta"), 0);
    }

    #[test]
    fn shorter_hypothesis_clamps_stable_len() {
        let mut la = LocalAgreement::new();
        la.push("one two three four");
        la.push("one two three four");
        let stable = la.push("one two");
        assert!(stable <= "one two".len());
        assert!("one two".is_char_boundary(stable));
    }

    #[test]
    fn multibyte_text_stays_on_char_boundaries() {
        let mut la = LocalAgreement::new();
        la.push("héllo wörld");
        let stable = la.push("héllo wörld again");
        assert!("héllo wörld again".is_char_boundary(stable));
        assert_eq!(&"héllo wörld again"[..stable], "héllo wörld");
    }

    #[test]
    fn reset_clears_history() {
        let mut la = LocalAgreement::new();
        la.push("same text");
        la.push("same text");
        la.reset();
        assert_eq!(la.push("same text"), 0);
    }
}
