//! Raw STT events → sentence-bounded [`Segment`]s.
//!
//! Partials pass through as single whole-utterance segments (the overlay
//! shows them verbatim); finals are split at sentence boundaries so cleanup
//! and insertion operate sentence-wise. Splitting here is intentionally
//! simple punctuation logic — abbreviation-aware refinement is the cleanup
//! chain's `Segmentation` processor (M5).

use std::time::Duration;

use od_core_types::{Segment, SegmentId, SegmentKind, SttEvent};

/// Stateful converter from [`SttEvent`]s to [`Segment`]s.
///
/// Id contract (see `od_core_types::Segment`): every partial of an utterance
/// carries the id that the utterance's *first* final sentence will get, so
/// UI surfaces can replace the provisional text in place.
#[derive(Debug, Default)]
pub struct Segmenter {
    next_id: u64,
    /// Id reserved by the current utterance's partials.
    reserved: Option<SegmentId>,
}

impl Segmenter {
    /// Creates a segmenter with ids starting at zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// Converts one STT event, appending produced segments to `out`.
    pub fn on_event(&mut self, event: &SttEvent, out: &mut Vec<Segment>) {
        match event {
            SttEvent::Partial { text, .. } => {
                let id = *self.reserved.get_or_insert_with(|| {
                    let id = SegmentId(self.next_id);
                    self.next_id += 1;
                    id
                });
                out.push(Segment {
                    id,
                    text: text.clone(),
                    kind: SegmentKind::Partial,
                    // Partial timing is unknown until finalization.
                    audio_start: Duration::ZERO,
                    audio_end: Duration::ZERO,
                });
            }
            SttEvent::Final { text, audio_len } => {
                let sentences = split_sentences(text);
                let total_bytes: usize = sentences.iter().map(|s| s.len()).sum();
                let mut consumed = 0usize;

                for (i, sentence) in sentences.iter().enumerate() {
                    // First sentence takes the partials' reserved id.
                    let id = if i == 0 {
                        self.reserved.take().unwrap_or_else(|| {
                            let id = SegmentId(self.next_id);
                            self.next_id += 1;
                            id
                        })
                    } else {
                        let id = SegmentId(self.next_id);
                        self.next_id += 1;
                        id
                    };

                    // Audio boundaries approximated proportionally by text
                    // share; real per-sentence timestamps come with token
                    // timing if a future backend provides them.
                    let start = proportion(*audio_len, consumed, total_bytes);
                    consumed += sentence.len();
                    let end = proportion(*audio_len, consumed, total_bytes);

                    out.push(Segment {
                        id,
                        text: sentence.clone(),
                        kind: SegmentKind::Final,
                        audio_start: start,
                        audio_end: end,
                    });
                }
                // Empty final (silence-only utterance): release the
                // reservation without emitting anything.
                if sentences.is_empty() {
                    self.reserved = None;
                }
            }
        }
    }
}

fn proportion(total: Duration, part: usize, whole: usize) -> Duration {
    if whole == 0 {
        return Duration::ZERO;
    }
    total.mul_f64(part as f64 / whole as f64)
}

/// Splits text into sentences after runs of `.`, `!`, `?` (keeping the
/// punctuation) when followed by whitespace. Trailing text without a
/// terminator is its own sentence.
fn split_sentences(text: &str) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    let mut sentences = Vec::new();
    let mut start = 0;
    let mut after_terminator = false;

    for (i, ch) in text.char_indices() {
        if matches!(ch, '.' | '!' | '?') {
            after_terminator = true;
        } else if after_terminator {
            if ch.is_whitespace() {
                let sentence = text[start..i].trim();
                if !sentence.is_empty() {
                    sentences.push(sentence.to_owned());
                }
                start = i;
            }
            after_terminator = false;
        }
    }
    let tail = text[start..].trim();
    if !tail.is_empty() {
        sentences.push(tail.to_owned());
    }
    sentences
}

#[cfg(test)]
mod tests {
    use super::*;

    fn final_event(text: &str, secs: f64) -> SttEvent {
        SttEvent::Final {
            text: text.into(),
            audio_len: Duration::from_secs_f64(secs),
        }
    }

    #[test]
    fn split_basic_sentences() {
        assert_eq!(
            split_sentences("Hello world. This is a test."),
            vec!["Hello world.", "This is a test."]
        );
    }

    #[test]
    fn split_keeps_unterminated_tail() {
        assert_eq!(
            split_sentences("First one. and then some"),
            vec!["First one.", "and then some"]
        );
    }

    #[test]
    fn split_handles_question_exclamation_and_runs() {
        assert_eq!(
            split_sentences("Really?! Yes. Wow..."),
            vec!["Really?!", "Yes.", "Wow..."]
        );
    }

    #[test]
    fn split_empty_and_whitespace() {
        assert!(split_sentences("").is_empty());
        assert!(split_sentences("   ").is_empty());
    }

    #[test]
    fn partial_then_final_shares_first_id() {
        let mut seg = Segmenter::new();
        let mut out = Vec::new();

        seg.on_event(
            &SttEvent::Partial {
                text: "hello wor".into(),
                stable_len: 0,
            },
            &mut out,
        );
        seg.on_event(&final_event("Hello world. Second sentence.", 4.0), &mut out);

        assert_eq!(out.len(), 3);
        assert_eq!(out[0].kind, SegmentKind::Partial);
        assert_eq!(out[1].kind, SegmentKind::Final);
        // First final reuses the partial's id; the next sentence gets a new one.
        assert_eq!(out[0].id, out[1].id);
        assert_ne!(out[1].id, out[2].id);
    }

    #[test]
    fn final_audio_times_partition_the_utterance() {
        let mut seg = Segmenter::new();
        let mut out = Vec::new();
        seg.on_event(&final_event("One. Two.", 2.0), &mut out);

        assert_eq!(out.len(), 2);
        assert_eq!(out[0].audio_start, Duration::ZERO);
        assert_eq!(out[0].audio_end, out[1].audio_start);
        assert_eq!(out[1].audio_end, Duration::from_secs(2));
    }

    #[test]
    fn ids_increase_across_utterances() {
        let mut seg = Segmenter::new();
        let mut out = Vec::new();
        seg.on_event(&final_event("First.", 1.0), &mut out);
        seg.on_event(&final_event("Second.", 1.0), &mut out);
        assert!(out[0].id < out[1].id);
    }

    #[test]
    fn empty_final_emits_nothing_and_releases_reservation() {
        let mut seg = Segmenter::new();
        let mut out = Vec::new();
        seg.on_event(
            &SttEvent::Partial {
                text: "noise".into(),
                stable_len: 0,
            },
            &mut out,
        );
        let partial_id = out[0].id;
        out.clear();

        seg.on_event(&final_event("", 0.5), &mut out);
        assert!(out.is_empty());

        // Next utterance must NOT inherit the stale reservation.
        seg.on_event(&final_event("Fresh.", 1.0), &mut out);
        assert_ne!(out[0].id, partial_id);
    }
}
