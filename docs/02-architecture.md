# 02 — Architecture

Scribbet is a single Rust process hosting a staged, actor-based audio→text pipeline,
with a thin Tauri/Svelte UI attached over IPC. Every stage sits behind a trait; the
default configuration uses zero ML beyond STT+VAD and zero network.

## System diagram

```
┌─────────────────────────────── Scribbet (Tauri app, one process) ───────────────────────────────┐
│                                                                                                     │
│  ┌───────────── webview ─────────────┐          ┌──────────────── Rust core ────────────────────┐  │
│  │ Svelte 5 UI                       │  events  │                                                │  │
│  │  • overlay pill (partial text)    │◄─────────│  tracing / metrics bus                         │  │
│  │  • settings window                │ commands │                                                │  │
│  │  • history browser                │─────────►│  session controller (state machine)            │  │
│  └───────────────────────────────────┘          │   ▲ hotkey mgr (RegisterHotKey + LL hook)      │  │
│                                                 │   │ tray icon (capture indicator)              │  │
│                                                 └───┼────────────────────────────────────────────┘  │
│                                                     │ spawns/owns pipeline actors (tokio tasks)      │
│  ┌──────────────────────────────────────────────────┼─────────────────────────────────────────────┐ │
│  │ PIPELINE (bounded mpsc channels between stages)  ▼                                             │ │
│  │                                                                                                │ │
│  │  mic ──► [audio capture] ──► ring buffer ──► [VAD] ──► [STT engine] ──► [segmenter]            │ │
│  │           cpal/WASAPI         SPSC, f32       Silero     whisper.cpp      partial vs final      │ │
│  │           16 kHz mono                         ONNX       (trait)          sentence bounds       │ │
│  │                                                                              │                 │ │
│  │                       final segments                                         ▼                 │ │
│  │  [insertion engine] ◄── [rewriter?] ◄── [cleanup chain] ◄── [command interpreter]              │ │
│  │   UIA → SendInput →      optional,        9 rule            dictation text vs                  │ │
│  │   clipboard-restore      OFF by default   processors        command? (grammar)                 │ │
│  │   (trait)                (trait)          (~µs total)          │ commands → [command executor] │ │
│  └────────────────────────────────────────────────────────────────┼───────────────────────────────┘ │
│                                                                   ▼                                 │
│  storage: SQLite (history, dictionary) · JSON settings · TOML profiles                              │
└─────────────────────────────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
                          target application (any focused window)
```

## Component walkthrough

### Session controller (`od-pipeline`)
Owns the global state machine: `Idle → Arming → Listening → Finalizing → Inserting → Idle`.
Transitions are driven by hotkey events (toggle / push-to-talk), VAD silence timeouts, and
insertion completion. It spawns the stage actors, wires bounded channels between them
(backpressure instead of unbounded memory growth), and publishes state + latency metrics
to the UI. Single owner of "are we recording" — the tray indicator can never disagree with
the mic state.

### Audio capture (`od-audio`)
cpal WASAPI input stream at the device's native rate, resampled to 16 kHz mono f32 (STT
native format). Writes into a lock-free SPSC ring buffer sized ~30 s. Exposes device
enumeration + hot-swap (mic selector) and an RMS level meter stream for the UI. The
capture thread does no allocation and no I/O — real-time safe.

### VAD (`od-vad`)
Silero v5 via ONNX Runtime (`ort` crate). Consumes 32 ms frames from the ring buffer,
emits `SpeechStart` / `SpeechEnd(t)` events with configurable hangover (default ~300 ms).
Purpose: (1) STT only runs on speech — CPU/battery win; (2) `SpeechEnd` triggers
finalization without waiting for the user to release the hotkey; (3) trims leading/trailing
silence so Whisper doesn't hallucinate on noise. ~1 MB model, <1 ms/frame on CPU.

### STT engine (`od-stt`)
```rust
trait SttEngine {
    fn begin_utterance(&mut self, hint: LanguageHint, bias: &VocabBias);
    fn feed(&mut self, samples: &[f32]) -> Vec<SttEvent>; // Partial | Final
    fn end_utterance(&mut self) -> Vec<SttEvent>;
}
```
Default backend: whisper.cpp (`whisper-rs`), `base.en` Q5 quantized, mmap-loaded on first
hotkey. Streaming is emulated via sliding-window incremental decode with local agreement
(a hypothesis is "stable" once two consecutive decodes agree on its prefix) — stable
prefixes become partials, `SpeechEnd` forces a final. Moonshine ONNX backend (natively
streaming, faster on CPU) slots in behind the same trait later. Custom dictionary terms
are fed as prompt bias per utterance.

### Segmenter (`od-pipeline`)
Turns raw STT events into sentence-bounded `Segment`s carrying text, timing, and
partial/final status. Partials flow to the overlay pill only; finals flow onward to the
command interpreter. Keeps a short tail buffer so sentence boundaries can be revised while
still partial.

### Command interpreter (`od-commands`)
Rule grammar (no ML) that classifies each final segment: dictation text vs command.
Commands: `new paragraph`, `new line`, `delete previous sentence/word`, `undo`,
`replace X with Y`, `select all`, `paste`, `stop dictating`. Grammar requires exact
command phrases as the *entire* segment (plus a configurable leading attention word,
e.g. "command, …") to avoid eating dictation that merely contains "undo". Commands go to
the executor (keystroke/UIA operations + our own undo stack of inserted spans); text goes
to cleanup.

### Cleanup chain (`od-cleanup`) — the rev-2 heart of the product
```rust
trait TextProcessor {
    fn name(&self) -> &'static str;
    fn process(&self, seg: &mut Segment, ctx: &PipelineCtx);
}
```
Ordered chain of pure-Rust processors, each toggleable/configurable per profile:

| # | Processor | What it does |
|---|---|---|
| 1 | `Whitespace` | collapse runs, trim, normalize unicode spaces |
| 2 | `FillerRemoval` | drop "um/uh/erm/you know/like" (list configurable; position-aware so "I like it" survives) |
| 3 | `Dictionary` | user vocabulary + jargon replacements (SQLite-backed, case-aware) |
| 4 | `Symbols` | spoken → symbol ("open brace"→`{`, "arrow"→`->`, "at sign"→`@`; table per profile) |
| 5 | `Punctuation` | repair STT punctuation: ensure terminal punctuation, fix comma splices around conjunction patterns, spoken punctuation ("comma", "period") |
| 6 | `Segmentation` | refine sentence splits/merges after punctuation repair |
| 7 | `Capitalization` | sentence-start, standalone "i", proper-noun dictionary, no-op inside code profile |
| 8 | `UserRules` | user-defined regex → replacement rules, ordered |
| 9 | `ProfileFormat` | profile-specific final pass (email: greeting/sign-off spacing; code: casing conventions like "camel case foo bar"→`fooBar`; meeting: bullet timestamps; etc.) |

Total cost: microseconds per segment. Every processor is a pure function over `Segment` —
golden-file table tests per processor and for the full chain.

### Rewriter (`od-rewrite`) — optional, OFF by default
```rust
trait Rewriter {
    fn rewrite(&self, seg: &Segment, ctx: &PipelineCtx) -> RewriteResult;
}
```
`RulesRewriter` (default) is an identity passthrough — the chain above already produced
final text, so the default pipeline pays nothing. `ClaudeRewriter`, `OpenAIRewriter`,
`LocalLLMRewriter` are future implementations behind cargo features and a runtime opt-in;
they are the only components that can introduce network access or extra model RAM.
Contract: rewriter failure/timeout ⇒ fall back to the cleaned text, never block insertion
beyond a configurable deadline.

### Insertion engine (`od-insertion`)
```rust
trait TextInserter {
    fn insert(&mut self, text: &str, target: &FocusInfo) -> InsertOutcome;
}
```
Windows backend, three tiers tried in order per target app:
1. **UIA TextPattern/ValuePattern** — semantic insertion at caret; also powers
   `delete previous sentence` by range manipulation.
2. **SendInput unicode events** — works in nearly everything, including apps with no UIA
   text support; rate-limited to avoid dropped keys in slow apps.
3. **Clipboard paste-and-restore** — set clipboard, send Ctrl+V, restore prior clipboard
   contents; last resort for paste-only fields and huge segments.

A per-app quirk table (by process name) pins the best tier and timing quirks (e.g.
terminals, RDP windows, Electron apps). Focus tracking captures the target *at hotkey
press*, so text lands where the user started even if focus flickers.

### Event bus (architectural concept — not implemented until stages exist)

Alongside the point-to-point stage channels (which carry the *data* flow: audio frames,
segments), the pipeline publishes **domain events** on a broadcast bus. Stage channels
are private plumbing; the event bus is the public, observable surface of the system —
the UI, metrics, history writer, and future plugins subscribe to it without touching the
hot path.

```rust
enum AppEvent {
    SpeechStarted   { t: Instant },
    SpeechEnded     { t: Instant, duration: Duration },
    PartialUpdated  { text: String, stable_prefix_len: usize },
    FinalReady      { segment_id: SegmentId, raw: String, cleaned: String },
    Inserted        { segment_id: SegmentId, tier: InsertionTier, latency: Duration },
    CommandExecuted { command: CommandKind, outcome: CommandOutcome },
    Undo            { segment_id: SegmentId },
    ProfileChanged  { from: ProfileId, to: ProfileId, cause: ProfileSwitchCause },
}
```

Rules: events are facts (past tense), never requests — publishing is fire-and-forget and
can never block a pipeline stage (bounded broadcast channel, slow subscribers drop and
log). Latency metrics fall out of event timestamps (`SpeechEnded` → `Inserted`).
Subscribers today: overlay pill, tray, history writer, metrics HUD. Subscribers later:
context detection, plugins — no pipeline changes needed to add them. Delivery lands
incrementally with the stages that emit each event (M2 onward); the enum above is the
contract.

### Profiles — the configuration unit

A profile is **not just a formatting mode**: it is the complete per-context configuration
bundle. Everything the pipeline consults that can vary by context lives in the active
profile:

```toml
# %APPDATA%/Scribbet/profiles/coding.toml
[profile]           # identity
name = "Coding"

[stt]               # engine hints
language = "en"
vocab_bias = ["dictionary:programming", "dictionary:user"]

[cleanup]           # processor chain configuration
fillers.enabled = true
capitalization.enabled = false        # code casing owns this
symbols.table = "programming"         # "open brace" -> {, "arrow" -> ->

[dictionaries]      # which dictionary sets apply
sets = ["user", "programming"]

[format]            # profile-specific final pass
casing_commands = true                # "camel case foo bar" -> fooBar

[cloud]             # cloud policy (see 06-security T2)
rewriter_allowed = false              # hard deny, even if a cloud rewriter is enabled globally

[plugins]           # future: per-profile plugin config (rewriter choice, params)
# rewriter = "claude"
```

The active profile is part of `PipelineCtx`, so every `TextProcessor`, the STT bias, and
the (optional) rewriter read from one coherent snapshot; switching profiles swaps the
snapshot atomically between utterances, never mid-segment. `ProfileChanged` is published
on the event bus. Shipped profiles (general, email, coding, meeting, professional,
medical, legal) are just TOML files — user profiles are the same format in the config
dir, no code required.

### Context detection (future capability — post-v1, documented for design headroom)

Automatic profile switching keyed on the focused application. The focus tracker in
`od-insertion` already observes the foreground window at hotkey press; context detection
extends that into a subscriber that maps focus info → profile:

```
focus info (process name, window class/title)
        │
        ▼
  rule table:  code.exe        → Coding
               discord.exe     → Casual
               winword.exe     → Professional
               outlook.exe     → Email
               (user-editable, first match wins)
        │
        ▼
  publish ProfileChanged { cause: ContextDetected }   (event bus)
```

Design constraints locked now so v1 doesn't paint us into a corner: profile switching is
already atomic-between-utterances (above); the switch *cause* is modeled in the event
(`Manual | ContextDetected`); manual selection always overrides detection for the current
session; and window titles are matched, never stored (see privacy posture). Not in v1 —
no implementation, no settings surface, only this contract.

### Storage (`od-storage`)
- **SQLite** (rusqlite, bundled): dictation history (text + timing + latency metrics,
  optional encrypt-at-rest decided M7), dictionary terms, symbol tables. Repository
  pattern: `HistoryRepo`, `DictionaryRepo` traits over a connection pool.
- **JSON settings** (serde): atomic write-temp-rename; no secrets ever stored here.
- **TOML profiles**: shippable defaults (general/email/coding/meeting/professional/
  medical/legal) + user profiles in the config dir. Full schema in "Profiles — the
  configuration unit" above: STT hints, processor config, dictionary sets, symbol table,
  format pass, cloud policy, future plugin config.

### UI (Tauri 2 + Svelte 5)
Three surfaces: borderless always-on-top **overlay pill** (partial text + level meter +
state color), **settings window** (profiles, dictionary CRUD, processor toggles, mic
selector, hotkeys, latency HUD), **history browser**. Rust owns all state; the UI is a
projection fed by events. Cold-start path never waits on the webview — hotkey and pipeline
are live before the UI finishes loading.

## Latency budget (speech-end → text inserted)

| Stage | Budget |
|---|---|
| VAD hangover (silence confirmation) | 200–300 ms (perceived as "natural pause", overlaps user silence) |
| STT finalize (tail decode) | 80–150 ms (base.en Q5, CPU) |
| Cleanup chain | <1 ms |
| Rewriter (default passthrough) | 0 ms |
| Insertion | 5–30 ms |
| **Total after user stops speaking** | **~120–300 ms** |

Perceived latency is lower still: partials are visible in the overlay *during* speech.

## Threading model

- cpal capture callback: real-time thread, ring-buffer write only.
- VAD + STT: dedicated compute thread (STT is CPU-bound; never on the tokio runtime).
- Pipeline actors (segmenter, cleanup, insertion, storage writes): tokio tasks, bounded
  mpsc channels.
- UI IPC: Tauri event loop.

## Failure policy

Every stage degrades, never blocks: VAD failure ⇒ pass-through (hotkey-gated capture
still works); rewriter timeout ⇒ insert cleaned text; UIA failure ⇒ next insertion tier;
insertion failure ⇒ text preserved in history + clipboard with a visible toast. The mic
is *always* released on `Idle`, enforced by the session controller owning the stream.
