//! Optional semantic rewriting — the extension seam, OFF by default (ADR-7).
//!
//! The default pipeline never rewrites: [`RulesRewriter`] passes the cleaned
//! text through untouched, so the default binary contains no HTTP client, no
//! TLS, and no model loader beyond STT/VAD. `ClaudeRewriter`,
//! `OpenAIRewriter`, and `LocalLLMRewriter` are post-v1, compiled only under
//! cargo features, and gated at runtime by the profile's cloud policy
//! (`ProfileSnapshot::rewriter_allowed`, docs/06 T2).

#![warn(missing_docs)]

use od_core_types::{PipelineCtx, Segment};

/// Outcome of a rewrite attempt.
///
/// Contract (docs/02): rewriter failure or timeout ⇒ the caller falls back
/// to the cleaned text and inserts it; a rewriter can never block insertion
/// beyond its deadline or lose text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RewriteResult {
    /// The segment text was left as-is (identity backends, no-op rewrites).
    Unchanged,
    /// The segment text was replaced with this rewrite.
    Rewritten(String),
    /// The backend failed or timed out; insert the cleaned text unchanged.
    Failed(String),
}

/// A semantic rewriting backend.
///
/// Implementations must respect the profile's cloud policy: when
/// `ctx.profile.rewriter_allowed` is false, network backends must return
/// [`RewriteResult::Unchanged`] without any I/O (hard deny, docs/06 T2).
pub trait Rewriter: Send {
    /// Stable identifier for logs and settings ("rules", "claude", ...).
    fn name(&self) -> &'static str;
    /// Proposes a rewrite for one cleaned segment.
    fn rewrite(&self, seg: &Segment, ctx: &PipelineCtx) -> RewriteResult;
}

/// The default identity backend: the cleanup chain already produced final
/// text, so the default pipeline pays nothing here.
pub struct RulesRewriter;

impl Rewriter for RulesRewriter {
    fn name(&self) -> &'static str {
        "rules"
    }

    fn rewrite(&self, _seg: &Segment, _ctx: &PipelineCtx) -> RewriteResult {
        RewriteResult::Unchanged
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use od_core_types::{SegmentId, SegmentKind};
    use std::time::Duration;

    #[test]
    fn rules_rewriter_is_identity() {
        let seg = Segment {
            id: SegmentId(1),
            text: "cleaned text.".into(),
            kind: SegmentKind::Final,
            audio_start: Duration::ZERO,
            audio_end: Duration::from_secs(1),
        };
        let r = RulesRewriter;
        assert_eq!(
            r.rewrite(&seg, &PipelineCtx::default()),
            RewriteResult::Unchanged
        );
        assert_eq!(r.name(), "rules");
    }
}
