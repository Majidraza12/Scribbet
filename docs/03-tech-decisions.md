# 03 — Technology Decisions (ADR digest)

One section per decision, ADR-style: context → decision → why → consequences.
Status of all: **accepted** (rev 2 plan approval, 2026-07-04).

## ADR-1 · App shell: Tauri 2 (not Electron, not pure native)

Idle footprint is a headline requirement (<120 MB RAM target). Electron ships a Chromium
per app (typ. 150–400 MB idle) — disqualified. Pure native (C#/WinUI or Swift) would fork
the codebase per OS and separate UI language from pipeline language. Tauri uses the OS
webview (WebView2 on Windows, already resident), and its host process *is* our Rust core —
pipeline and shell share one binary. Consequence: webview quirks differ per OS; UI kept
deliberately thin so this stays cheap.

## ADR-2 · Core language: Rust

Real-time audio thread safety, zero-GC latency floor, first-class FFI to ggml/ONNX,
single language for pipeline + Tauri host + future macOS/Linux ports. Consequence:
slower initial velocity than C#; accepted for a long-lived owned product.

## ADR-3 · Audio: cpal (not PortAudio)

Rust-native, WASAPI backend, device enumeration/hot-swap, no C build dependency.
PortAudio adds an FFI layer and a vendored C lib for zero additional capability here.
Consequence: on Linux later, cpal's ALSA/JACK story needs validation (accepted risk).

## ADR-4 · VAD: Silero v5 via ONNX Runtime (not WebRTC VAD)

WebRTC VAD is energy/GMM-based: cheap but noisy — false triggers on keyboard/breath,
clipped word onsets. Silero is ~1 MB, <1 ms/frame on one core, and dramatically more
accurate; accuracy here directly gates STT CPU burn and finalization latency. `ort`
crate is needed anyway if the Moonshine STT backend lands (shared dependency).
Consequence: ONNX Runtime adds ~15–20 MB to install size — acceptable.

## ADR-5 · STT default: whisper.cpp / `base.en` Q5 (not faster-whisper, not Parakeet)

- **faster-whisper**: best throughput, but Python runtime — disqualified for a lean
  Rust desktop binary.
- **NVIDIA Parakeet**: superb accuracy/speed *on NVIDIA GPUs*; violates CPU-baseline.
- **Whisper large-v3**: too slow on CPU for streaming feel.
- **whisper.cpp**: pure C/C++, GGML quantization, mmap model loading (fast cold start,
  RAM shared with page cache), opportunistic Vulkan/CUDA offload, mature Rust bindings
  (`whisper-rs`). `base.en` Q5 ≈ 60 MB disk, real-time+ on modern laptop CPU; `small`
  offered as opt-in quality bump.
- **Moonshine** (ONNX): natively streaming, faster than Whisper on CPU; planned second
  backend behind `SttEngine`, not the default until we validate accuracy on our fixtures.

Consequence: whisper.cpp streaming is emulated (sliding window + local agreement), which
costs some CPU during speech; bounded by VAD gating.

## ADR-6 · Cleanup: rule chain, zero models (rev 2 core decision)

Original draft put a local 1.7B LLM in the default path. Rejected: +1–2 GB disk,
+1.5 GB RAM when hot, +300–800 ms per sentence, worse battery — for cleanup that rules
achieve at ~0 cost for the dominant cases (fillers, casing, punctuation repair,
vocabulary, symbols). Decision: 9-processor `TextProcessor` chain (see 02-architecture)
is the *only* default cleanup. Consequence: no semantic rewriting (tone shifts, heavy
grammar surgery) by default — that's exactly what the opt-in `Rewriter` trait is for.

## ADR-7 · Rewriter: trait + feature-gated backends, OFF by default

`Rewriter` trait ships with identity `RulesRewriter`. `ClaudeRewriter` (claude-haiku-4-5
class models: fast, cheap, ideal for rewrite tasks), `OpenAIRewriter`
(OpenAI-compatible endpoints, which also covers local servers like llama.cpp/ollama in
server mode), and `LocalLLMRewriter` (in-process llama.cpp plugin) are **post-v1**,
compiled only under cargo features. Consequence: default binary contains no HTTP client,
no TLS, no model loader beyond STT/VAD — auditable privacy claim.

## ADR-8 · Insertion: UIA → SendInput → clipboard-restore (3 tiers)

No single Windows text-injection mechanism covers all apps. UIA TextPattern is semantic
(and enables voice *editing*), SendInput unicode covers ~everything with a caret,
clipboard-paste is the universal last resort. Per-app quirk table (process name → tier +
timing) handles known offenders (terminals, RDP, some Electron apps). Consequence:
clipboard tier must snapshot & restore user clipboard — handled, see 06-security.

## ADR-9 · Hotkeys: RegisterHotKey + low-level keyboard hook

RegisterHotKey for the toggle chord (cheap, global). A scoped WH_KEYBOARD_LL hook only
while push-to-talk is armed, to detect key-*up* (RegisterHotKey can't). Hook is installed
lazily and removed at Idle to keep idle CPU at zero.

## ADR-10 · Storage: SQLite (rusqlite, bundled) + JSON settings + TOML profiles

SQLite for anything queryable (history, dictionary, metrics) via repository traits;
`bundled` feature avoids system-lib drift. Settings = single JSON file, serde,
atomic temp-file-rename writes, no secrets. Profiles = TOML (comments + human editing
matter for user-extensible profiles). Secrets (future cloud keys) → Windows Credential
Manager via `keyring`, compiled only with cloud features.

## ADR-11 · State management: Rust owns truth, UI is a projection

All app state lives in the Rust core (session state machine + settings struct). Svelte 5
runes hold only view state, hydrated by Tauri events. No state duplication, no UI-side
persistence. Consequence: every user action is an IPC command; fine at our event rates.

## ADR-12 · Logging: `tracing`, content-free by default

`tracing` with rolling file appender in the app data dir. Spans model pipeline stages —
latency metrics fall out of span timings. Transcribed *content* is never logged at
default level; a debug flag enables it locally with a visible warning. UI log level
switch for support cases.

## ADR-13 · Crash reporting: local minidumps, opt-in Sentry

`minidumper`-style local crash dumps always on (user can attach to a bug report).
Network crash reporting (Sentry) is compile-time optional and runtime opt-in,
consistent with the no-telemetry default.

## ADR-14 · Updates: Tauri updater, signed, GitHub Releases

Signed update manifests (Tauri v2 updater), release channel on GitHub Releases.
Update *check* is a network call: performed only on explicit user action or when
auto-update is enabled in settings (single toggle; off = zero network, consistent
with ADR-7). Models are not part of app updates — downloaded separately with checksum
verification to `%LOCALAPPDATA%/OpenDictate/models`.

## ADR-15 · UI stack: Svelte 5 + TypeScript

Compiled output, no VDOM runtime, smallest webview payload → fastest overlay/settings
start, which protects the <2 s cold-start budget. User-confirmed choice over React/Solid.

## ADR-16 · Cargo workspace with one crate per pipeline stage (not a single crate)

**Context.** The pipeline has ~10 components with sharply different dependency
footprints: `od-stt` pulls whisper.cpp (C++ build, long compile), `od-vad` pulls ONNX
Runtime, `od-insertion` pulls windows-rs, while `od-cleanup`/`od-commands` are pure Rust
with near-zero deps. A single crate would fuse all of that into one compilation unit.

**Decision.** One workspace, one crate per stage, boundaries matching the trait seams
(`SttEngine`, `TextProcessor`, `Rewriter`, `TextInserter`).

**Why.**
- *Build isolation*: editing a cleanup rule must not recompile whisper.cpp FFI. Iteration
  speed on the pure-logic crates (where most of the test-driven work happens) stays in
  seconds.
- *Dependency hygiene*: `cargo tree -p od-cleanup` proves the cleanup chain has no ML or
  network deps — the privacy claim in 06-security is auditable per crate, and a heavy
  dep can't silently leak into a lean crate.
- *Enforced layering*: crates can only use what they declare; a single crate lets any
  module reach any other. The trait seams stay real because the compiler polices them.
- *Portability triage*: CI tests the portable crates (`cleanup`, `commands`, `rewrite`,
  `storage`, `core-types`) on Linux today; platform crates (`insertion`, `audio` backends)
  are cleanly quarantined for the cross-platform phase.
- *Feature gating*: cargo features for cloud rewriters (ADR-7) attach to `od-rewrite`
  alone instead of threading through a monolith.

**Consequences.** More `Cargo.toml` ceremony; shared types must live in `od-core-types`
(a deliberate chokepoint — churn there is a design smell we want to feel). Workspace
`[workspace.dependencies]` keeps versions pinned once.

## ADR-17 · Windows-first, cross-platform by trait seam (not simultaneous multi-OS)

**Context.** The end goal is cross-platform, but the OS-coupled surface — text insertion,
global hotkeys, tray/overlay, credential storage — is where dictation apps live or die,
and it cannot be abstracted well *before* one concrete implementation exists. The
developer's machine and the underserved market (see 01-product-analysis) are both Windows.

**Decision.** Ship v1 on Windows only. Every OS-coupled component is defined by a
portable trait in a portable crate, with the Windows backend as the first implementation:
`TextInserter` (UIA/SendInput/clipboard today; macOS AX API, Linux AT-SPI later),
hotkey manager, capture backend (cpal already abstracts this), secret store (`keyring`
already abstracts this).

**Why this supports incremental porting rather than blocking it.**
- The *portable core* (pipeline, cleanup, commands, storage, STT, VAD — the large
  majority of the code) is kept provably OS-free by the workspace structure (ADR-16) and
  the Linux CI job from day one. Porting = writing new backend crates, not refactoring.
- Trait seams designed against one *real* backend beat speculative abstractions designed
  against zero: the Windows quirk table will teach us what the insertion trait actually
  needs (timing, focus semantics, tier fallback) before we freeze it for three OSes.
- Milestones stay small and shippable; a simultaneous 3-OS build would triple the
  M3/M4 surface (hotkeys, insertion) — the two hardest, most platform-specific
  milestones — before any user value ships.

**Consequences.** macOS/Linux users wait; some Windows assumptions may still leak through
the seams and surface during porting (accepted — cheaper than speculative design). The
Tauri/Svelte UI layer is cross-platform from day one, so porting cost is concentrated in
`od-insertion` + hotkey/tray backends.
