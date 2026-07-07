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

## M8 — Packaging ◐
- [x] Tauri bundler: MSI + NSIS targets active (NSIS per-user install,
      WebView2 download bootstrapper). **Unsigned** — code-signing needs a
      certificate the project doesn't own yet; blocked on user, tracked below.
- [x] Onboarding window (shown when the model is missing instead of the old
      exit(2)): privacy/mic rationale, model download with resume (HTTP
      Range → .part file), pinned SHA-256 verification before the file may
      carry the model name, restart button. The app's only network code path
      (docs/06 posture table updated).
- [x] Updater: **not shipped in v1 by decision** — ADR-19 (docs/03). An
      unsigned auto-update channel is worse than none (T7); manual installer
      upgrades + published checksums until cert + hosting exist.
- [x] Code signing: **dropped by user decision 2026-07-06** — app is
      local-only, not distributed, so a certificate serves no purpose
      (signing only suppresses SmartScreen warnings for third-party
      downloads). Revisit only if distribution ever becomes a goal.
- [ ] Manual voice matrix from M4 (needs user's microphone) — still owed
- [ ] Review gate

## M9 — Testing hardening ◐
- [x] E2E suite (`apps/desktop/src-tauri/tests/e2e.rs`, `#[ignore]`d):
      WAV fixture → VAD → whisper → segmenter → cleanup chain →
      WindowsInserter → real EDIT window → read back. Both tests green with
      the exact cleaned transcript. Safety hardened after I-7 (docs/04):
      foreground re-verified before *every* insert.
- [x] Soak test `scripts/soak-test.ps1` (10 min default, quick mode):
      release run — RAM 123.6 MB max (target 120 soft / 250 hard, see
      docs/04 note), CPU 0.00 % avg. Found I-8 (second-launch panic) →
      fixed with tauri-plugin-single-instance.
- [x] Fuzz-style property tests (proptest, CI-safe, 512 cases each):
      cleanup chain total + tidy on arbitrary unicode, arbitrary user regex
      rules never panic, chain accepts its own output; profile TOML /
      settings JSON parsers total; parsed profiles always resolve.
      (Command grammar fuzz n/a — M6 skipped.)
- [x] Supply chain in CI: cargo-deny job (advisories, license allow-list,
      source bans, wildcard bans) + `/deny.toml`.
- [ ] Review gate

## M10 — v1.0 release ◐
- [x] P1-1 (STT finalize ≤300 ms p50): investigated at M10 — the whisper.cpp
      `audio_ctx` shortcut is unstable on base.en-q5_1 (data in docs/04);
      finalize cost is fixed encoder cost, so the target is unreachable with
      this backend on CPU. **Closed 2026-07-06 by user decision**: v1 ships
      with ~1.0–1.2 s finalize (partials already stream live); ≤300 ms p50
      re-scoped to the post-v1 Moonshine streaming backend (docs/04 closure
      entry).
- [x] Perf validation vs targets (release build, docs/04 M8 column): cold
      start 305 ms (target ≤2 s) · idle RAM 123.6 MB (120 soft/250 hard,
      accepted note) · idle CPU 0.00 % (≤5 %) · hotkey→listening 51 ms
      (≤100 ms) · overruns 0 · fixture transcripts exact end-to-end.
      Finalize acceptance run pending an idle machine (see docs/04 P1-1).
- [x] User docs: README install/use/build sections; onboarding covers
      first-run (model download, privacy, hotkeys).
- [ ] Manual voice matrix (M4 debt — needs the user's microphone).
      **Known gap at tag time** (user decision 2026-07-06): trails the tag,
      to be run when the user has mic time; automated e2e (real EDIT window,
      exact cleaned transcripts) covers the insertion path meanwhile.
- [x] Tag v1.0.0 — tagged 2026-07-06 with P1-1 closed, voice matrix
      trailing, and code signing dropped (local-only use — M8).

## v1.1 — GPU acceleration ☑ (2026-07-06)
Trigger: first real daily use — 2–3 s perceived lag unusable for natural
dictation; machine turned out to have an RTX 4060 the CPU-baseline
assumption ignored.
- [x] `od-stt/vulkan` → `opendictate-desktop/gpu-vulkan` features (ADR-20);
      whisper on GPU, runtime CPU fallback keeps the binary universal
- [x] Finalize 196 ms measured (was ~1200 ms) — **P1-1 target met**;
      transcripts exact; idle RAM ~112 MB (model in VRAM)
- [x] GPU builds: `decode_interval` 700 → 300 ms default (feature-gated)
- [x] Build env: Vulkan SDK 1.4.350.0; `CARGO_TARGET_DIR=C:\odt`
      (MSVC FileTracker MAX_PATH — docs/04 v1.1 note); CI stays CPU
- [x] User-validated live ("actually working, impressed") incl. VS Code

## v1.2 — Wispr-style UX + session insertion ☑ (2026-07-07)
Driven by first days of real daily use.
- [x] Overlay redesign: permanent tiny black pill (outline, white bars, no
      text, no green), bottom-center 52 px above taskbar; expands while
      listening; click-to-talk (`toggle_session` command). Fixed the
      shared-CSS leak that painted the transparent overlay window dark
      (`:global(body)` from Settings/Onboarding — I-11).
- [x] One dictation session = one insert (finals buffered, inserted at stop)
      and one history row (`AppEvent::SessionCompleted`); clipboard fallback
      when every tier fails, so text is never lost.
- [x] I-9: stop-click focused the pill → text typed into the overlay.
      Fix: focus capture resolves our own windows to the window beneath
      (z-order walk) + focus handback before typing.
- [x] I-10: UIA SetValue reports success against hidden accessibility nodes
      in Electron/Chromium apps (Cursor). Fix: quirk table prefers SendInput
      for cursor/code/chrome/edge/discord/slack/etc.
- [x] Perf: whisper flash-attention on GPU builds — finalize 196 → 160 ms;
      fixtures exact.

## Post-v1 (parked, by design)
- ⏸ `ClaudeRewriter` / `OpenAIRewriter` / `LocalLLMRewriter` (cargo features, per ADR-7)
- ⏸ Moonshine ONNX streaming STT backend
- ⏸ Wake phrase (idle-CPU budget conflict)
- ⏸ macOS (AX API insertion) / Linux (AT-SPI, wlroots) backends
- ⏸ Profile sharing/import (treat as untrusted input, see 06-security T6)
