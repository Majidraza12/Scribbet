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
- [x] **REVIEW GATE 1** — passed 2026-07-04

## M2 — Streaming transcription ◐
- [x] `od-vad`: Silero v5 via `ort` (model embedded, 2.3 MB); `VadGate` state
      machine: hysteresis, min-speech blip filter, hangover; sample-offset events
- [x] `od-stt`: `SttEngine` trait; whisper.cpp backend (whisper-rs 0.16), base.en Q5
- [x] Re-decode cadence (700 ms) + `LocalAgreement` word-boundary stable prefixes
- [x] `od-core-types`: SegmentId/Segment/SttEvent/LanguageHint/VocabBias/PipelineCtx
- [x] Segmenter (sentence splitting, id reservation partial→final) + `Transcriber`
      (VAD-gated engine feeding, pre-roll, finalize-latency metric) in `od-pipeline`
- [x] Models: scripts/fetch-models.ps1 (whisper, SHA-256 pinned); TTS fixture
      generator scripts/gen-fixtures.ps1 + 4 committed fixtures
- [x] Tests: 13 CI-safe unit (agreement/segmenter/core-types) + 5 VAD fixture tests
      + 4 mock-engine transcriber tests + 3 `#[ignore]`d whisper e2e (all pass;
      transcripts exact on all fixtures)
- [x] Perf: finalize latency 1.2 s logged as **P1-1** in docs/04-engineering-log.md
      with acceptance criteria (≤300 ms p50 on fixtures); blocks v1, not M3
- [x] **REVIEW GATE 2** — passed 2026-07-04 (with engineering refinements:
      per-milestone perf baselines, P1 tracking, issue log, hot-path rules —
      see docs/04-engineering-log.md)

## M3 — Global hotkey + first UI ◐
- [x] Hotkeys via tauri-plugin-global-shortcut (Pressed/Released covers
      push-to-talk without a raw LL hook): Ctrl+Shift+Space toggle,
      Ctrl+Shift+D PTT (defaults; configurable M7)
- [x] Session controller thread (`od-pipeline`): Idle (blocks, 0 CPU) /
      Listening / Finalizing; single owner of the capture session (T1);
      `Inserting` state joins in M4 as planned
- [x] `EventBus` (bounded, drop-on-full broadcast) + `AppEvent` contract in
      `od-core-types` (serde-tagged for the UI bridge)
- [x] Tauri 2 app in workspace: tray (state tooltip, quit menu), transparent
      always-on-top overlay pill (state dot, 12-bar level meter, stable/partial
      text), Svelte 5 UI (37 KB built), event bridge thread, `get_level` poll
- [x] Perf (docs/04 table): cold start 523 ms, hotkey→listening 51 ms,
      idle 0 % CPU / 129 MB WS (debug), live-verified via synthesized hotkey
      presses against the running app
- [x] **REVIEW GATE 3** — passed 2026-07-04

## M4 — Universal insertion ◐
- [x] `od-insertion`: `TextInserter` trait + `NullInserter` (display-only) +
      `WindowsInserter`
- [x] Tier 1 UIA ValuePattern (empty-field fast path + editability/password
      probe; caret-to-end after SetValue — I-5) · Tier 2 SendInput unicode
      (newline→Return, surrogate pairs, modifier-remnant release) · Tier 3
      clipboard swap → Ctrl+V → restore (retry-on-locked, non-text warned)
- [x] Focus captured at hotkey press; re-verified at insert; follows user's
      deliberate focus moves; password fields never transit the clipboard
- [x] Per-app quirk table (terminals/RDP prefer clipboard tier, settle delays)
- [x] Controller integration: finals inserted live during dictation;
      `Inserted`/`InsertFailed` events; display-only fallback
- [x] Tests: 3 CI-safe unit + 2 `#[ignore]`d harness tests against a real
      EDIT window (exact 2-insert ordering incl. unicode/newline; clipboard
      sentinel restore), with foreground guard (I-6). Live app run verified
      focus capture + session cycle
- [ ] Manual matrix by voice (Notepad, VS Code, Chrome, Word, Slack,
      Windows Terminal) — needs the user's microphone; **deferred by user
      decision 2026-07-05 to run alongside M5+; still owed before M8**
- [x] **REVIEW GATE 4** — automated review passed 2026-07-05 (tests,
      perf table, engineering log); manual voice matrix outstanding, above

Note: docs/02 listed an `Inserting` session state; insertion happens inline
per-final on the controller thread (a few ms) so no separate state was
needed — finals insert *during* Listening, which is the better UX anyway.

## M5 — Cleanup chain & profiles ◐
- [x] `od-cleanup`: `TextProcessor` trait + chain runner (built per profile,
      one `cleanup` debug event per segment with `chain_us`)
- [x] Processors 1–9 (whitespace, fillers, dictionary, symbols, punctuation,
      segmentation, capitalization, user rules, profile format); position-aware
      fillers ("I like it" survives), determiner guard for spoken punctuation
      ("the period of time" survives), casing commands ("camel case foo bar")
- [x] `od-rewrite`: `Rewriter` trait + `RulesRewriter` passthrough — extension
      point only; deliberately *not* called by the controller (identity would
      be pure overhead; wiring lands with the first real backend, post-v1)
- [x] `od-storage`: dictionary repo (SQLite via rusqlite bundled, schema v1),
      TOML profile loader (user dir shadows shipped; unknown keys rejected),
      atomic JSON settings (temp-file + rename)
- [x] Shipped profiles: general, email, coding, meeting, professional,
      medical, legal (embedded TOML; medical/legal subscribe to dictionary
      sets the user fills via the M7 editor)
- [x] STT vocab bias from dictionary (bias toward *written* forms); language
      wiring: profile `stt.language` = "auto" → `LanguageHint::Auto`
- [x] Controller: finals run raw → chain → insertion; `FinalReady` now
      carries `raw` + `cleaned`; overlay shows cleaned text
- [x] Tests: golden-file tables per processor + full chain (33), storage (17),
      rewrite (1); workspace total 108 CI-safe + 7 `#[ignore]`d
- [x] Perf: chain 6.0 µs/segment (release, 7 active processors) — docs/04
- [ ] Review gate ← next stop

Note (M5 deviations): meeting profile emits plain "- " bullets — the docs/02
"bullet timestamps" idea needs a wall clock, which would make processor 9
impure/untestable; revisit at M7 with the history writer. `Inserting` note
from M4 still applies: cleanup runs inline per-final on the controller
thread (µs), no pipeline stage added.

## M6 — Voice commands ⏸ (skipped by user decision, 2026-07-05)
User reviewed the command set and chose pure transcription; the commands were judged
not useful for their workflow. `od-commands` stays a scaffold crate; the docs/02
interpreter design remains valid if this is ever revisited post-v1.
- ⏸ `od-commands`: grammar + interpreter — parked
- ⏸ Command executor + insertion-span undo stack — parked

## M7 — Settings UI + history ◐
- [x] Settings window (Tauri `settings` window, Svelte, routed on window label):
      profile selector, cleanup processor toggles + format flags (saved as user
      TOML shadow, hot-swapped into the live pipeline), dictionary CRUD ("user"
      set, re-resolves vocab bias live), mic selector (device list, next-session
      swap), hotkey capture (parse-validated re-registration; bad strings can
      never leave the app hotkey-less), latency HUD (cold start, finalize,
      insertion tier, counters)
- [x] History: `od-storage::SqliteHistoryRepo` (same DB file as the dictionary,
      own connection), written by the event bridge on `FinalReady`, browser tab
      with filter/copy, purge button, enable toggle + cap in settings
- [x] Pipeline: `SessionCommand::UpdateCtx`/`SetDevice` (applied idle or after
      the current session — never mid-utterance), `AppEvent::UtteranceFinalized`
      (HUD metric) + `AppEvent::ProfileChanged`
- [x] SQLCipher decision: **no encryption in v1** — ADR-18 (docs/03); docs/06
      updated. No passphrase exists, so a key would sit next to the DB; NTFS
      ACLs + opt-out + purge are the honest mitigations.
- [x] Tests: history repo (5), profile save/round-trip; workspace 115 CI-safe
      green; clippy/fmt clean; app smoke run (profile resolve, hotkeys from
      settings file, cold start 728 ms debug)
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
- [ ] All P1 items in docs/04-engineering-log.md closed against their
      acceptance criteria (P1-1: STT finalize ≤300 ms p50)
- [ ] Perf validation vs all targets (cold start, latency, RAM, CPU)
- [ ] User docs, README polish
- [ ] Tag v1.0.0

## Post-v1 (parked, by design)
- ⏸ `ClaudeRewriter` / `OpenAIRewriter` / `LocalLLMRewriter` (cargo features, per ADR-7)
- ⏸ Moonshine ONNX streaming STT backend
- ⏸ Wake phrase (idle-CPU budget conflict)
- ⏸ macOS (AX API insertion) / Linux (AT-SPI, wlroots) backends
- ⏸ Profile sharing/import (treat as untrusted input, see 06-security T6)
