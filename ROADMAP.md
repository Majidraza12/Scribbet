# OpenDictate — Roadmap & Living Checklist

Status legend: ☐ todo · ◐ in progress · ☑ done · ⏸ deferred
Every milestone ends with a **review gate**: code + tests presented, user approves before
the next milestone starts. This file is updated at every milestone boundary.

## Deliverable 1 — Docs & scaffold ◐
- [x] git init, `main` branch
- [x] Cargo workspace, 10 crate stubs (`crates/*`), shared workspace deps
- [x] CI skeleton (fmt, clippy `-D warnings`, tests; Windows + portable-crates Linux job)
- [x] docs/01-product-analysis.md — category matrix, wedge, non-goals, MVP success criteria
- [x] docs/02-architecture.md — system diagram, component walkthrough, event bus contract,
      profile schema, context detection (future), latency budget
- [x] docs/03-tech-decisions.md — ADR-1…17
- [x] docs/06-security.md — trust boundaries TB1–TB5, threat model T1–T8, privacy posture
- [x] ROADMAP.md (this file), README.md
- [x] `cargo build` + `cargo test` + fmt + clippy green locally
- [x] Initial commit
- [x] **REVIEW GATE 0** — passed 2026-07-04

## M1 — Audio capture ◐
- [x] `od-audio`: cpal WASAPI stream, 16 kHz mono f32 resample
      (rational-phase linear resampler, exactly chunk-invariant)
- [x] Lock-free SPSC ring buffer (rtrb, 30 s, drop-newest + overrun counter),
      allocation-free capture callback
- [x] Device enumeration, selection; hot-swap = session restart; unplug surfaces
      via `is_disconnected()` (cpal has no device-change events — documented)
- [x] RMS level meter (atomic, instant attack / exponential release; UI polls)
- [x] Tests: 21 unit (dsp/ring/meter/device) + 2 integration (synthetic graph,
      WAV round-trip) + `#[ignore]`d live-mic smoke + `record_wav` example
- [x] Live verification: 2 s capture from real device, 48 kHz stereo → 16 kHz
      mono, 0 overruns
- [ ] **REVIEW GATE 1** ← next stop

## M2 — Streaming transcription ☐
- [ ] `od-vad`: Silero v5 via `ort`; SpeechStart/SpeechEnd + hangover config
- [ ] `od-stt`: `SttEngine` trait; whisper.cpp backend (`whisper-rs`), base.en Q5
- [ ] Sliding-window incremental decode + local-agreement partials
- [ ] `od-core-types`: Segment, SttEvent, PipelineCtx finalized
- [ ] Segmenter (partial/final sentence bounds) in `od-pipeline`
- [ ] Model download-with-checksum helper (dev-time; user-facing flow lands M8)
- [ ] Tests: fixture WAVs → expected transcripts; latency assertions; VAD gate tests
- [ ] Review gate

## M3 — Global hotkey + first UI ☐
- [ ] Hotkey manager: RegisterHotKey toggle + LL-hook push-to-talk (lazy hook)
- [ ] Session controller state machine (Idle/Arming/Listening/Finalizing/Inserting)
- [ ] Tauri app joins workspace (`apps/desktop`): tray icon (capture indicator),
      overlay pill (partials + level meter + state color)
- [ ] Svelte 5 scaffold, event bridge Rust→UI
- [ ] Cold-start budget check: hotkey live <2 s
- [ ] Review gate

## M4 — Universal insertion ☐
- [ ] `od-insertion`: `TextInserter` trait
- [ ] Tier 1 UIA TextPattern/ValuePattern · Tier 2 SendInput unicode · Tier 3
      clipboard-paste-restore
- [ ] Focus capture at hotkey press; per-app quirk table
- [ ] Tests: automated harness window; manual matrix (Notepad, VS Code, Chrome, Word, Slack)
- [ ] Review gate

## M5 — Cleanup chain & profiles ☐
- [ ] `od-cleanup`: `TextProcessor` trait + chain runner
- [ ] Processors 1–9 (whitespace, fillers, dictionary, symbols, punctuation,
      segmentation, capitalization, user rules, profile format)
- [ ] `od-rewrite`: `Rewriter` trait + `RulesRewriter` passthrough (extension point only)
- [ ] `od-storage`: dictionary repo (SQLite), TOML profile loader, JSON settings
- [ ] Shipped profiles: general, email, coding, meeting, professional, medical, legal
- [ ] STT vocab bias from dictionary; language auto-detect wiring
- [ ] Tests: golden-file tables per processor + full chain; profile round-trip
- [ ] Review gate

## M6 — Voice commands ☐
- [ ] `od-commands`: grammar + interpreter (new paragraph/line, delete previous
      sentence/word, undo, replace X with Y, select all, paste, stop dictating)
- [ ] Command executor: UIA/keystroke ops, insertion-span undo stack
- [ ] Tests: grammar table tests, ambiguity cases ("I like undo buttons" ≠ command)
- [ ] Review gate

## M7 — Settings UI + history ☐
- [ ] Settings window: profiles editor, dictionary CRUD, processor toggles,
      mic selector, hotkey capture, latency HUD
- [ ] History browser (app-inserted items), purge, caps
- [ ] SQLCipher encrypt-at-rest decision (perf-test then default or setting)
- [ ] Review gate

## M8 — Packaging ☐
- [ ] Tauri bundler: MSI + NSIS, signed
- [ ] Onboarding: mic permission rationale, model download (checksum, resume)
- [ ] Signed updater (opt-in check), release channel
- [ ] Review gate

## M9 — Testing hardening ☐
- [ ] E2E suite (synthetic audio → focused test app → assert inserted text)
- [ ] Soak test: 10 min idle asserts RAM <120 MB target/<250 MB hard, CPU <5%
- [ ] Fuzz: command grammar, cleanup chain, profile/TOML parsers
- [ ] `cargo audit`/`cargo deny` in CI
- [ ] Review gate

## M10 — v1.0 release ☐
- [ ] Perf validation vs all targets (cold start, latency, RAM, CPU)
- [ ] User docs, README polish
- [ ] Tag v1.0.0

## Post-v1 (parked, by design)
- ⏸ `ClaudeRewriter` / `OpenAIRewriter` / `LocalLLMRewriter` (cargo features, per ADR-7)
- ⏸ Moonshine ONNX streaming STT backend
- ⏸ Wake phrase (idle-CPU budget conflict)
- ⏸ macOS (AX API insertion) / Linux (AT-SPI, wlroots) backends
- ⏸ Profile sharing/import (treat as untrusted input, see 06-security T6)
