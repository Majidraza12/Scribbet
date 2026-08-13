# Scribbet

Local-first AI dictation for the desktop. Hold a hotkey, speak naturally, and polished
text is inserted into whatever application has focus — offline, private, and light enough
to forget it's running.

Windows-first; core is portable Rust with platform backends behind traits.

## Why

Every polished dictation product today makes you choose: cloud quality with your voice
leaving the machine, or local privacy with raw robotic transcripts. Scribbet's default
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
~0% idle CPU · zero network code in the default build. The one exception:
first-run onboarding downloads the STT model, checksum-pinned — that is the
app's only network path, ever.

## Using it

| Action | Default |
|---|---|
| Toggle dictation | `Ctrl+Shift+Space` |
| Push-to-talk | `Ctrl+Shift+D` |
| Click the overlay pill | toggle |

A small translucent pill sits bottom-center above the taskbar: presence
indicator when idle, live level meter while listening. Hotkeys, cleanup
profiles, custom dictionary, mic, STT model, and history are all configurable
in the Settings window (tray icon).

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

## Installing

No prebuilt binaries yet — build from source (a few minutes on a normal
machine).

**The easy way:** clone this repo, open [Claude Code](https://claude.com/claude-code)
in it, and paste the prompt at the top of [`BOOTSTRAP.md`](BOOTSTRAP.md). It
installs the missing toolchain, builds the right variant for your hardware
(GPU via Vulkan, or CPU), runs the installer, and verifies the overlay comes
up.

**By hand:** follow the same file — every step is a plain PowerShell command.

The binary is unsigned; SmartScreen warns on first interactive run
(More info → Run anyway).

## Developing

Requires Rust stable (see `rust-toolchain.toml`) and, on Windows, MSVC Build Tools.

```
cargo build --workspace
cargo test  --workspace
```

For the full desktop app build (installer bundles, GPU features, environment
quirks), see [`BOOTSTRAP.md`](BOOTSTRAP.md).

## Documentation

- [Product analysis](docs/01-product-analysis.md) — category study and positioning
- [Architecture](docs/02-architecture.md) — diagrams, component walkthrough, latency budget
- [Tech decisions](docs/03-tech-decisions.md) — ADR-1…20
- [Engineering log](docs/04-engineering-log.md) — perf baselines and the issue log, unedited
- [Security](docs/06-security.md) — threat model and privacy posture

## Status

**v1.2.1 — daily driver.** Shipped and in full-time use as the author's
replacement for Wispr Flow. Highlights since v1.0: GPU (Vulkan) whisper with
~200–400 ms finalize on a mid-range card and runtime CPU fallback,
session-batched insertion with per-app quirk handling for Electron/Chromium
hosts, the always-on overlay pill with click-to-talk, configurable STT model
(`base.en` default, `large-v3-turbo` validated), instant stop on long
monologues. Voice commands (M6) were cut by product decision. History:
[ROADMAP.md](ROADMAP.md) and the [engineering log](docs/04-engineering-log.md).

## License

[MIT](LICENSE)
