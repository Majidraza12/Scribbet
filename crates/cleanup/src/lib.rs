//! Rule-based text cleanup — the rev-2 heart of the product (ADR-6).
//!
//! An ordered chain of nine pure-Rust processors turns raw STT text into
//! insertable text in microseconds, with zero models and zero network. Each
//! processor is a pure function over [`Segment`]; the set and configuration
//! come from the active profile snapshot in [`PipelineCtx`].
//!
//! Chain order is fixed (docs/02-architecture.md "Cleanup chain"):
//! whitespace → fillers → dictionary → symbols → punctuation → segmentation
//! → capitalization → user rules → profile format. Profiles toggle and
//! configure processors; they cannot reorder them.

#![warn(missing_docs)]

mod capitalization;
mod dictionary;
mod fillers;
mod phrase;
mod profile_format;
mod punctuation;
mod segmentation;
mod symbols;
mod user_rules;
mod whitespace;

pub use capitalization::Capitalization;
pub use dictionary::Dictionary;
pub use fillers::FillerRemoval;
pub use profile_format::ProfileFormat;
pub use punctuation::Punctuation;
pub use segmentation::Segmentation;
pub use symbols::Symbols;
pub use user_rules::UserRules;
pub use whitespace::Whitespace;

use std::time::Instant;

use od_core_types::{PipelineCtx, Segment};

/// One step of the cleanup chain. Implementations must be pure over the
/// segment text: no I/O, no clocks, no global state — that is what makes
/// golden-file table tests sufficient.
pub trait TextProcessor: Send {
    /// Stable identifier used in logs and the settings UI.
    fn name(&self) -> &'static str;
    /// Transforms the segment text in place.
    fn process(&self, seg: &mut Segment, ctx: &PipelineCtx);
}

/// The ordered processor chain, built from a profile snapshot.
///
/// Building compiles user-rule regexes and resolves tables once; running is
/// allocation-light and costs microseconds per segment. Rebuild the chain
/// when the active profile changes (snapshots swap between utterances, so a
/// chain never sees a profile change mid-segment).
pub struct Chain {
    procs: Vec<Box<dyn TextProcessor>>,
}

impl Chain {
    /// Builds the chain enabled/configured per `ctx.profile.cleanup`.
    pub fn from_ctx(ctx: &PipelineCtx) -> Self {
        let cfg = &ctx.profile.cleanup;
        let mut procs: Vec<Box<dyn TextProcessor>> = Vec::new();
        if cfg.whitespace {
            procs.push(Box::new(Whitespace));
        }
        if cfg.fillers.enabled {
            procs.push(Box::new(FillerRemoval::new(&cfg.fillers)));
        }
        if cfg.dictionary && !ctx.profile.entries.is_empty() {
            procs.push(Box::new(Dictionary::new(&ctx.profile.entries)));
        }
        if cfg.symbols.enabled {
            procs.push(Box::new(Symbols::from_table(&cfg.symbols.table)));
        }
        if cfg.punctuation.enabled {
            procs.push(Box::new(Punctuation::new(cfg.punctuation.clone())));
        }
        if cfg.segmentation {
            procs.push(Box::new(Segmentation));
        }
        if cfg.capitalization {
            procs.push(Box::new(Capitalization::new(&cfg.proper_nouns)));
        }
        if !cfg.user_rules.is_empty() {
            procs.push(Box::new(UserRules::new(&cfg.user_rules)));
        }
        if cfg.format != od_core_types::FormatConfig::default() {
            procs.push(Box::new(ProfileFormat::new(cfg.format.clone())));
        }
        Self { procs }
    }

    /// Runs every processor in order over one segment. Emits a single
    /// `cleanup` debug event with the chain cost in microseconds
    /// (per-segment, per docs/04 instrumentation convention).
    pub fn run(&self, seg: &mut Segment, ctx: &PipelineCtx) {
        let t0 = Instant::now();
        for p in &self.procs {
            p.process(seg, ctx);
        }
        tracing::debug!(
            chain_us = t0.elapsed().as_micros() as u64,
            processors = self.procs.len(),
            "cleanup"
        );
    }

    /// Names of the active processors, in order (logs, settings UI).
    pub fn names(&self) -> Vec<&'static str> {
        self.procs.iter().map(|p| p.name()).collect()
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use std::time::Duration;

    use od_core_types::{PipelineCtx, Segment, SegmentId, SegmentKind};

    use crate::TextProcessor;

    /// Runs one processor over `input` with a default context.
    pub fn apply(proc: &dyn TextProcessor, input: &str) -> String {
        apply_ctx(proc, input, &PipelineCtx::default())
    }

    /// Runs one processor over `input` with the given context.
    pub fn apply_ctx(proc: &dyn TextProcessor, input: &str, ctx: &PipelineCtx) -> String {
        let mut seg = Segment {
            id: SegmentId(1),
            text: input.to_owned(),
            kind: SegmentKind::Final,
            audio_start: Duration::ZERO,
            audio_end: Duration::from_secs(1),
        };
        proc.process(&mut seg, ctx);
        seg.text
    }

    /// Asserts a table of (input, expected) golden cases for one processor.
    pub fn golden(proc: &dyn TextProcessor, cases: &[(&str, &str)]) {
        for (input, expected) in cases {
            let got = apply(proc, input);
            assert_eq!(
                &got,
                expected,
                "processor `{}` on input {input:?}",
                proc.name()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use od_core_types::{
        CleanupConfig, DictEntry, FormatConfig, ProfileSnapshot, SymbolsConfig, UserRule,
    };
    use std::sync::Arc;
    use std::time::Duration;

    fn ctx_with(cleanup: CleanupConfig, entries: Vec<DictEntry>) -> PipelineCtx {
        PipelineCtx {
            profile: Arc::new(ProfileSnapshot {
                name: "Test".into(),
                cleanup,
                entries,
                ..ProfileSnapshot::default()
            }),
            ..PipelineCtx::default()
        }
    }

    fn run_chain(ctx: &PipelineCtx, input: &str) -> String {
        let chain = Chain::from_ctx(ctx);
        let mut seg = Segment {
            id: od_core_types::SegmentId(1),
            text: input.to_owned(),
            kind: od_core_types::SegmentKind::Final,
            audio_start: Duration::ZERO,
            audio_end: Duration::from_secs(1),
        };
        chain.run(&mut seg, ctx);
        seg.text
    }

    #[test]
    fn default_chain_full_pass() {
        let ctx = ctx_with(CleanupConfig::default(), Vec::new());
        // whitespace + fillers + spoken punctuation + capitalization together
        assert_eq!(
            run_chain(&ctx, "um  so i\u{a0}think, like, this   works comma right"),
            "So I think this works, right."
        );
    }

    #[test]
    fn default_chain_enables_expected_processors() {
        let ctx = ctx_with(CleanupConfig::default(), Vec::new());
        let chain = Chain::from_ctx(&ctx);
        // No dictionary entries and default format => both skipped.
        assert_eq!(
            chain.names(),
            vec![
                "whitespace",
                "fillers",
                "symbols",
                "punctuation",
                "segmentation",
                "capitalization"
            ]
        );
    }

    #[test]
    fn coding_style_chain() {
        let cfg = CleanupConfig {
            capitalization: false,
            symbols: SymbolsConfig {
                enabled: true,
                table: "programming".into(),
            },
            format: FormatConfig {
                casing_commands: true,
                ..FormatConfig::default()
            },
            ..CleanupConfig::default()
        };
        let ctx = ctx_with(cfg, Vec::new());
        assert_eq!(
            run_chain(
                &ctx,
                "let camel case user profile service equals sign open brace"
            ),
            "let userProfileService = {"
        );
    }

    #[test]
    fn dictionary_and_user_rules_apply_in_order() {
        let cfg = CleanupConfig {
            user_rules: vec![UserRule {
                pattern: r"\bgonna\b".into(),
                replacement: "going to".into(),
            }],
            ..CleanupConfig::default()
        };
        let entries = vec![DictEntry {
            spoken: "open dictate".into(),
            written: "OpenDictate".into(),
            case_sensitive: false,
        }];
        let ctx = ctx_with(cfg, entries);
        assert_eq!(
            run_chain(&ctx, "open dictate is gonna ship"),
            "OpenDictate is going to ship."
        );
    }

    #[test]
    #[ignore = "perf measurement, not an assertion; run release with --ignored --nocapture"]
    fn chain_cost_measurement() {
        let entries = vec![DictEntry {
            spoken: "open dictate".into(),
            written: "OpenDictate".into(),
            case_sensitive: false,
        }];
        let ctx = ctx_with(CleanupConfig::default(), entries);
        let chain = Chain::from_ctx(&ctx);
        let input = "um so i think, like, open dictate is gonna work comma \
                     however i'm not sure about the timing period";
        let mut seg = Segment {
            id: od_core_types::SegmentId(1),
            text: input.to_owned(),
            kind: od_core_types::SegmentKind::Final,
            audio_start: Duration::ZERO,
            audio_end: Duration::from_secs(3),
        };
        const N: u32 = 10_000;
        let t0 = std::time::Instant::now();
        for _ in 0..N {
            seg.text.clear();
            seg.text.push_str(input);
            chain.run(&mut seg, &ctx);
        }
        let total = t0.elapsed();
        println!(
            "cleanup chain: {:.1} us/segment over {N} runs ({} processors); output: {:?}",
            total.as_micros() as f64 / f64::from(N),
            chain.names().len(),
            seg.text
        );
    }

    #[test]
    fn empty_segment_survives_chain() {
        let ctx = ctx_with(CleanupConfig::default(), Vec::new());
        assert_eq!(run_chain(&ctx, ""), "");
        assert_eq!(run_chain(&ctx, "   "), "");
    }
}
