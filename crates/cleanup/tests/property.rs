//! M9 fuzz-style property tests: the cleanup chain must be total — any
//! unicode garbage in, no panic out, and the whitespace/format invariants
//! hold. STT output is untrusted-ish input (mumbles, hallucinated tokens,
//! foreign scripts), and user regex rules are fully untrusted.

use std::time::Duration;

use od_cleanup::Chain;
use od_core_types::{PipelineCtx, ProfileSnapshot, Segment, SegmentId, SegmentKind, UserRule};
use proptest::prelude::*;

fn segment(text: &str) -> Segment {
    Segment {
        id: SegmentId(1),
        text: text.to_owned(),
        kind: SegmentKind::Final,
        audio_start: Duration::ZERO,
        audio_end: Duration::from_secs(1),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Default profile (format passes off): arbitrary input never panics,
    /// and the output is trimmed with no internal double spaces and no
    /// newlines the input didn't earn (email/bullets are off).
    #[test]
    fn default_chain_is_total_and_tidy(input in any::<String>()) {
        let ctx = PipelineCtx::default();
        let chain = Chain::from_ctx(&ctx);
        let mut seg = segment(&input);
        chain.run(&mut seg, &ctx);

        prop_assert_eq!(seg.text.trim(), seg.text.as_str(), "output not trimmed");
        prop_assert!(!seg.text.contains("  "), "double space in {:?}", seg.text);
        prop_assert!(!seg.text.contains('\n'), "newline without a format pass: {:?}", seg.text);
        prop_assert!(!seg.text.contains('\0'), "NUL survived cleanup");
    }

    /// User rules are untrusted: arbitrary (frequently invalid) regex
    /// patterns and replacements must never panic — invalid patterns are
    /// skipped at chain build time by contract.
    #[test]
    fn arbitrary_user_rules_never_panic(
        pattern in any::<String>(),
        replacement in any::<String>(),
        input in any::<String>(),
    ) {
        let mut snapshot = ProfileSnapshot::default();
        snapshot.cleanup.user_rules = vec![UserRule { pattern, replacement }];
        let ctx = PipelineCtx {
            profile: std::sync::Arc::new(snapshot),
            ..PipelineCtx::default()
        };
        let chain = Chain::from_ctx(&ctx);
        let mut seg = segment(&input);
        chain.run(&mut seg, &ctx);
        // Reaching here without a panic is the property.
    }

    /// Running the chain on its own output must not panic either (users
    /// re-dictate corrected text that already went through cleanup once).
    #[test]
    fn chain_accepts_its_own_output(input in any::<String>()) {
        let ctx = PipelineCtx::default();
        let chain = Chain::from_ctx(&ctx);
        let mut seg = segment(&input);
        chain.run(&mut seg, &ctx);
        let once = seg.text.clone();
        chain.run(&mut seg, &ctx);
        // And tidiness still holds after a second pass.
        prop_assert_eq!(seg.text.trim(), seg.text.as_str());
        let _ = once;
    }
}
