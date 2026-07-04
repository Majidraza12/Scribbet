# OpenDictate

Local-first AI dictation for the desktop. Hold a hotkey, speak naturally, and polished
text is inserted into whatever application has focus — offline, private, and light enough
to forget it's running.

> Working name. Windows-first; core is portable Rust with platform backends behind traits.

## Why

Every polished dictation product today makes you choose: cloud quality with your voice
leaving the machine, or local privacy with raw robotic transcripts. OpenDictate's default
pipeline is **entirely local and model-minimal** — modern speech recognition plus a
rule-based cleanup chain (punctuation, casing, filler removal, custom vocabulary,
programming symbols) that costs microseconds, not gigabytes. LLM rewriting exists as a
strictly opt-in plugin, never a dependency.

## Pipeline

```
mic → VAD (Silero) → STT (whisper.cpp) → cleanup chain (9 rule processors)
    → [optional rewriter, OFF by default] → universal insertion (UIA/SendInput/clipboard)
```

Design goals: <300 ms perceived latency · <2 s cold start · <120 MB idle RAM ·
~0% idle CPU · zero network code in the default build.

## Repository layout

```
crates/
  core-types/   shared types, events, PipelineCtx
  audio/        cpal capture, ring buffer, devices
  vad/          Silero ONNX gate
  stt/          SttEngine trait + whisper.cpp backend
  cleanup/      TextProcessor chain (the default "AI")
  rewrite/      optional Rewriter trait (passthrough default)
  commands/     voice-command grammar + executor
  insertion/    TextInserter trait + Windows backend
  pipeline/     session controller, actors, metrics
  storage/      SQLite repos, JSON settings, TOML profiles
apps/desktop/   Tauri 2 + Svelte 5 shell (joins at Milestone 3)
docs/           product analysis · architecture · ADRs · threat model
ROADMAP.md      living milestone checklist
```

## Building

Requires Rust stable (see `rust-toolchain.toml`) and, on Windows, MSVC Build Tools.

```
cargo build --workspace
cargo test  --workspace
```

## Documentation

- [Product analysis](docs/01-product-analysis.md) — category study and positioning
- [Architecture](docs/02-architecture.md) — diagrams, component walkthrough, latency budget
- [Tech decisions](docs/03-tech-decisions.md) — ADR-1…15
- [Security](docs/06-security.md) — threat model and privacy posture

## Status

Pre-alpha: documentation and workspace scaffold (Deliverable 1). See
[ROADMAP.md](ROADMAP.md) for the milestone plan.
