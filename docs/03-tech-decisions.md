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
