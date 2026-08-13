# 04 — Engineering Log

Running log of significant issues, performance baselines, and engineering
conventions. Updated every milestone. Format for issues: root cause → why it
happened → why the fix is correct → follow-up.

## Conventions

### Hot-path rules (binding)

The audio/transcription hot path (capture callback → ring → VAD → STT feed)
must not gain:
- allocations in steady state (pre-sized scratch buffers only; growth is a
  one-time warm-up event, never per-callback),
- locks or channels with blocking sends (atomics and wait-free rings only;
  the capture callback communicates via `AtomicU32`/`AtomicU64` and rtrb),
- extra sample copies (current copy count per chunk: device→mono scratch→
  resampled scratch→ring→window→engine buffer; do not add more),
- blocking I/O or logging of per-frame events (`tracing` events on the hot
  path are per-*utterance* or on error, never per-frame/per-callback).

Review question for every pipeline PR: "what does this add to the per-chunk
cost?"

### Instrumentation convention

Stage boundaries emit `tracing` events with millisecond fields rather than
per-frame spans (span-per-frame would itself violate the hot-path rules):
- `od-audio`: overrun warnings (dropped/total), session start/stop info.
- `od-vad`: `speech start` / `speech end` debug events with sample offsets.
- `od-stt`: `whisper decode` debug event — `audio_ms`, `decode_ms` per decode.
- `od-pipeline`: `utterance finalized` info event — `finalize_ms` (headline
  metric, also exposed as `Transcriber::last_finalize_latency()`).
- M3+: session controller state transitions (info), hotkey→listening ms,
  cold-start ms; cleanup chain and insertion get per-segment ms fields.

Rule: every stage that can add user-perceivable latency must expose its cost
as a numeric field on exactly one event per unit of work.

## P1 performance items (must close before v1)

| # | Item | Current | Acceptance criteria | Tracked since |
|---|---|---|---|---|
| P1-1 | **MET 2026-07-06 on GPU builds** (see v1.1 entry below). STT finalization latency: `end_utterance` re-decodes the whole utterance | **196 ms** (release, `gpu-vulkan` build, RTX 4060) · ~1.0–1.2 s on CPU-only builds (accepted for CPU baseline) | ≤ 300 ms p50 / ≤ 500 ms p95 on the fixture set — met by the Vulkan backend; unreachable on CPU (M10 investigation below), where Moonshine remains the post-v1 path. | M2 |

(New P1 items get a row here; nothing may be silently dropped from this table
— items leave it only by meeting their acceptance criteria or by an explicit
user decision recorded in this file.)

### P1-1 investigation (M10, 2026-07-05): audio_ctx shortcut rejected with data

The docs/02 latency budget assumed finalize cost scales with utterance length.
It does not: whisper.cpp pads every input to a 30 s window, so the ~1 s
finalize is almost all *fixed* encoder cost — which also means the "decode
only the tail at SpeechEnd" candidate fix cannot work (a 0.7 s tail decode
costs the same ~1 s).

The one CPU knob that does cut encoder cost, `audio_ctx` (attend only the
frames the clip fills — whisper.cpp's own streaming trick), was implemented
and rejected on quality: with base.en-q5_1, every configuration tested
(tight ctx ±slack; floors 64–512; with/without `single_segment`;
with/without `no_timestamps`) either produced repeat-loop garbage
("2 2 2 …", dropped tails) on 2 of 3 fixtures, or — at ctx 512+ with
single_segment — kept transcripts exact but made the decoder wander the
in-window silence, costing ~3× baseline. Truncated positional context is
simply unstable on this quantized model. The rejected code is preserved in
this entry and a NOTE at the decode site.

Consequence: **≤300 ms p50 finalize is not reachable with full-window
whisper.cpp base.en on the CPU baseline.** The realistic paths are (a) the
Moonshine ONNX streaming backend (already parked post-v1, ADR-5 seam ready),
or (b) accepting ~1.0–1.2 s finalize for v1 (partials already stream live,
so the user sees text within the cadence interval; the final merely commits
it). Closing or re-scoping P1-1 is a user decision, pending.

### P1-1 closure (2026-07-06): user accepted ~1.0–1.2 s finalize for v1

User decision at the v1.0.0 gate: option (b) — ship v1 with ~1.0–1.2 s
finalize. Rationale: partials stream live within the 700 ms cadence, so the
final only commits already-visible text; the alternative (Moonshine backend)
is a new-backend effort that would hold the release for a metric users mostly
don't perceive. The ≤300 ms p50 target is re-scoped to the post-v1 Moonshine
ONNX streaming backend (ADR-5 seam ready) and leaves this table per the
explicit-decision rule above.

### P1-1 resolution (2026-07-06, v1.1): Vulkan GPU backend — target met

Hours after the closure above, live use on the user's machine surfaced that
2–3 s perceived lag makes natural dictation unusable. Investigation (idle
16-thread machine, so not the I-3 contention trap) reconfirmed the fixed CPU
encoder cost — and hardware inventory found an RTX 4060 the "CPU baseline"
assumption had been ignoring.

Fix: whisper-rs `vulkan` feature, exposed as `od-stt/vulkan` →
`scribbet-desktop/gpu-vulkan` (ADR-20). No source changes to the decode
path — `WhisperContextParameters::default()` enables GPU when the feature is
compiled in, and ggml falls back to CPU at runtime when no Vulkan device
exists, so the GPU binary is safe everywhere.

Measured (whisper_e2e serial, RTX 4060): finalize **196 ms** vs ~1200 ms CPU
— p50 target met with margin; transcripts stay exact on all fixtures. The
cheap decodes also allowed `decode_interval` 700 ms → 300 ms on GPU builds
(feature-conditional default in `WhisperConfig`), tightening partial-text
tracking during continuous speech. Idle RAM dropped to ~112 MB WS (model
weights now resident in VRAM), back under the 120 MB soft target.

Build note: MSVC FileTracker hits MAX_PATH (FTK1011) on the Vulkan shader
generator's nested paths under `C:\Coding V2\internal_tool\target`; GPU
builds use `CARGO_TARGET_DIR=C:\odt`. Vulkan SDK 1.4.350.0 is a build-time
dep only (CI stays CPU; runtime Vulkan comes from the GPU driver).

Lesson (I-3 family): "CPU baseline" was a design assumption carried past the
point where checking the actual deployment hardware would have paid for
itself immediately.

Note on numbers: re-measurement during this investigation was contaminated —
the dev machine was concurrently running a game plus Folding@home (baseline
code measured 11.7 s vs its historical 1.2 s). Timing conclusions above rely
on the clean earlier baselines and on *relative* behavior; the formal
acceptance run must happen on an idle machine (I-3 lesson, extended: check
system load before trusting any latency number).

## Performance baselines per milestone

Numbers from the dev machine (4-core+ laptop-class CPU, no GPU assumed).
"n/a" = the surface doesn't exist yet at that milestone.

| Metric | M1 | M2 | M3 | M8 (release) | v1.1 (gpu-vulkan, RTX 4060) | Target (v1) |
|---|---|---|---|---|---|---|
| Cold start (process → hotkey live) | n/a | n/a | **523 ms** (debug build, eager model load) | **305 ms** | **367 ms** | ≤ 2 s |
| Idle RAM | n/a | n/a | **129 MB** WS (debug; whisper model resident ≈ 59 MB of that) | **123.6 MB** WS (see note) | **~112 MB** WS (model in VRAM) | ≤ 120 MB (250 hard) |
| Idle CPU | n/a | n/a | **0 %** over 10 s sample | **0.00 %** avg over soak | 0 % | ≤ 5 % |
| STT partial cadence | n/a | 700 ms configured | unchanged | unchanged | **300 ms** (feature-conditional default) | partial visible ≤ 500 ms behind voice |
| STT finalize (speech-end → final) | n/a | ~1200 ms (P1-1) | unchanged (P1-1 open) | unchanged (P1-1 → M10) | **196 ms** (P1-1 target met); **160 ms** at v1.2 (flash attention) | ≤ 300 ms p50 (met on GPU; CPU builds accepted at ~1.0–1.2 s) |
| Hotkey → listening | n/a | n/a | **51 ms** (toggle → mic open + Listening published) | unchanged | unchanged | ≤ 100 ms |
| Ring overruns during capture | 0 (2 s live) | 0 (fixtures) | 0 (live session) | 0 | 0 | 0 |
| Transcript accuracy on fixtures | n/a | exact on 4/4 | unchanged | exact (e2e, incl. cleaned insertion) | exact | exact |

M8 release measurements: `scripts/soak-test.ps1` against
`target/release/scribbet-desktop.exe`. Idle RAM note: 123.6 MB is 3 % over
the 120 MB soft target and 2× inside the 250 MB hard ceiling; ~59 MB of it is
the eagerly resident whisper model — deliberate, because lazy loading would
push model-load latency into the user's first utterance. Settings/onboarding
webviews are created lazily (saved ~3 MB and two idle WebView2 processes).
Accepted as the v1 number; recorded here so it can't silently drift.

M3 collection method: `RUST_LOG=info` run of the debug app; `cold_start_ms` and
`hotkey_to_listening_ms` tracing fields; `Get-Process` WS + CPU delta over 10 s
idle. Watch item (not yet P1): idle RAM is 9 MB over the 120 MB aim in a debug
build with the model eagerly resident — re-measure on a release build at M8
before deciding whether lazy/mmap model residency work is needed.

M4 additions (insertion, harness-measured on real windows): UIA ValuePattern
tier ~0.9–2.4 ms per segment; SendInput tier ~1.7–4.4 ms for a ~25-char
segment; both far inside the budget (docs/02 allots 5–30 ms). Live app run:
hotkey→listening 32 ms with focus capture included. Clipboard tier cost is
dominated by the deliberate 150–400 ms settle delay (quirk-configured) — only
paid in terminal/RDP targets or as last resort.

M5 additions (cleanup chain): **6.0 µs/segment** (release, default profile
with one dictionary entry = 7 active processors, 108-char segment, 10k-run
average via `chain_cost_measurement` — run with
`cargo test -p od-cleanup --release chain_cost_measurement -- --ignored --nocapture`).
Budget in docs/02 is <1 ms: three orders of magnitude of headroom. The chain
runs inline per-final on the controller thread and emits one `cleanup` debug
event with `chain_us` per segment (instrumentation convention holds). Profile
resolve + SQLite dictionary load happen once at startup, not per segment;
smoke run: cold start 578 ms debug (523 ms at M3 — the +55 ms is
settings/profile/SQLite init, well inside the ≤2 s target); idle RAM/CPU
re-measure on release build at M8 as planned.

M7 additions (settings + history): cold start **728 ms** debug (578 ms at M5) —
the +150 ms is the second (settings) webview window plus the history SQLite
connection; still 2.7× inside the ≤2 s target, release re-measure stays
scheduled for M8. History writes happen on the event-bridge thread (one INSERT
+ capped DELETE per final, off the controller thread — hot-path rules
untouched). Profile/context hot-swap (`SessionCommand::UpdateCtx`) applies only
while idle or between sessions, so no per-segment cost was added. New HUD
surface: settings window polls `get_perf` at 1.5 s — no event-bus subscriber
added, no pipeline instrumentation changed. M6 was skipped by user decision
(2026-07-05, ROADMAP) — no perf surface.

## Issue log

### I-13 (v1.2.1, PARTIAL 2026-07-08) · Inconsistent insertion into a terminal *embedded in an Electron host* (Cursor's integrated terminal)

- **Symptom**: dictating into the integrated terminal inside Cursor lands the
  text only *sometimes*; single-clicking the terminal (or double-clicking)
  right before each start/stop makes it reliable. Reported on an AMD iGPU
  laptop (Balanced power plan); the same build on the reporter's desktop had
  never shown it.
- **Two distinct causes, one fixed here, one inherent**:
  1. **Fixed** — the re-activation path used a single `SetForegroundWindow`
     plus a fixed 80 ms settle before typing/pasting. Under CPU throttling the
     target did not become foreground within 80 ms, so a click-to-talk stop
     inserted into nothing. Replaced with `focus::activate_and_wait` — polls
     until the target truly owns the foreground (nudging the foreground lock
     with a synthetic Alt tap on retry, the I-6 workaround) plus a short
     Electron focus-routing settle. Confirmed via captured logs: click-to-talk
     stops now insert (~145–170 ms) where they previously vanished.
  2. **Fixed (2026-07-08, follow-up)** — user pinned a sharper pattern: short
     dictations always land, long ones ("talk for a while") drop. Cause: text
     over `LONG_TEXT_PASTE_CHARS` (120) was promoted to the Clipboard tier
     (`Ctrl+V`), and a paste into Cursor's embedded terminal is exactly the
     unreliable path — the clipboard is restored before the pane consumes the
     paste. Short text used SendInput and landed. Fix: never promote clipboard
     for hosts that prefer SendInput (the Electron/Chromium set); keep typing
     regardless of length, with clipboard only as a fallback, plus a 6 ms
     per-batch keystroke pacing so a long typed burst isn't dropped by the
     terminal's render loop.
  3. **Inherent / unresolved** — the inserter targets the foreground *window*
     (`cursor.exe`); it cannot see or choose which pane inside that single
     Chromium process holds the keyboard (terminal vs editor). When focus has
     drifted to the editor pane, a successful `SendInput`/paste lands there,
     not the terminal — the app logs `inserted` (the OS accepted the events)
     yet the user sees nothing in the terminal. No Win32 handle exists for the
     child pane to target or verify.
- **Evidence**: a 9-dictation session logged zero insertion failures — every
  attempt reported `inserted tier=… app=cursor.exe`. The perceived failures
  occur *after* a successful hand-off, i.e. pane-level focus, invisible to the
  insertion layer.
- **Workaround (documented for users)**: click the terminal pane, then toggle
  with the **hotkey** (clicking the pill is the action most likely to disturb
  pane focus); or use a standalone terminal (`WindowsTerminal.exe`), a separate
  process with the proper Clipboard quirk and no pane ambiguity.
- **Follow-up (candidates, not yet done)**: (a) after activation, resolve the
  focused UIA element and, if its bounding rect is a descendant of the target
  window, `SetFocus`/`Invoke` to re-assert the intended pane before typing;
  (b) an optional per-app "always paste + extra settle" override for
  `cursor.exe`; (c) reporter is dogfooding for a few days and will bring
  back frequency data before deciding whether (a)/(b) are worth the risk.

### I-12 (v1.2.1, RESOLVED 2026-07-07) · Taskbar button of the running app still shows the old icon

- **Status**: RESOLVED — reboot cleared it. Machine rebooted 2026-07-07
  2:11 PM (a day earlier than planned); after reboot, with the settings
  window open, the running app's taskbar button shows the new orange icon
  (verified visually via taskbar screenshot).
- **Root cause**: stale Windows shell *session* icon state. The exe
  resource, runtime window icon, and shortcut were all already correct;
  the shell kept serving the old bitmap for the rest of that logon session
  even after icon-cache DB deletion + Explorer restart + `ie4uinit -show`.
  Only a reboot flushed it.
- **Verified so far**: `apps/desktop/src-tauri/icons/*` regenerated from the
  new logo (`npx tauri icon`); first bundle rebuild did NOT re-embed the
  icon because no Rust input changed (cargo skipped the resource step) —
  fixed by touching `build.rs` and rebuilding; the installed exe's embedded
  resource icon is confirmed orange (extracted via
  `[System.Drawing.Icon]::ExtractAssociatedIcon` and inspected). Icon cache
  DBs deleted + Explorer restarted + `ie4uinit -show`; pin/unpin tried.
  Taskbar button still shows the old icon within the same Windows session.
- **Post-reboot verification (2026-07-07)**: running process confirmed to be
  the installed exe (`%LOCALAPPDATA%\Scribbet\scribbet-desktop.exe`);
  runtime window icon confirmed orange via `WM_GETICON` on the live window
  (rules out the stale-`generate_context!`-bytes suspect); Start-Menu `.lnk`
  icon is `,0` (exe resource, orange); no stale pinned-taskbar `.lnk`
  exists. Settings window opened (second-launch single-instance forward)
  and its taskbar button screenshot shows the new orange mic icon.
- **Incidental finding**: the overlay pill window carries `WS_EX_APPWINDOW`
  (not `WS_EX_TOOLWINDOW`); Tauri's skipTaskbar hides it via
  `ITaskbarList::DeleteTab` instead. Harmless, noted so nobody "fixes" the
  style later expecting it to control taskbar presence.

### I-9 (v1.2) · Stop-click on the overlay pill typed the text into the pill

- **Symptom**: sessions stopped via click-to-talk inserted nowhere visible;
  log showed `focus moved since capture … to=scribbet-desktop.exe`.
- **Root cause**: clicking the pill makes the overlay the foreground window;
  the inserter's by-design "follow the user's focus" rule then treated our
  own pill as the user's chosen target.
- **Why it happened**: click-to-talk was added (v1.2 overlay) without
  revisiting the focus contract, which was written when hotkeys were the
  only trigger. (`focusable: false` was tried first and rejected: Windows
  then delivers no clicks to the webview at all — verified empirically.)
- **Why the fix is correct**: our own windows can never be a dictation
  target, so `focus::capture` resolves any own-process foreground to the
  first visible, titled, non-tool foreign window below it in the z-order,
  and `insert` hands the foreground back to the effective target (allowed —
  we own the foreground in exactly this case) before any keystroke tier
  runs. Verified live: click-started and click-stopped sessions insert into
  the window under the pill.
- **Follow-up**: none; hotkey-only flow is unchanged (foreground never
  moves, both checks are no-ops).

### I-10 (v1.2) · UIA SetValue "succeeds" invisibly in Electron/Chromium apps

- **Symptom**: `inserted tier=Uia … app=cursor.exe` in the log, nothing on
  screen; user saw the text vanish.
- **Root cause**: Chromium's UIA tree exposes writable value-patterned
  nodes that aren't the visible editor; `SetValue` against them returns
  success without rendering anywhere the user looks.
- **Why it happened**: the UIA fast path was validated against Win32 EDIT
  controls (harness, Notepad), where ValuePattern is truthful. Web-rendered
  accessibility trees break the "success means visible" assumption.
- **Why the fix is correct**: the quirk table now prefers SendInput for
  known Electron/Chromium hosts (cursor, code, chrome, msedge, brave,
  firefox, discord, slack, notion, obsidian, teams) — synthetic keystrokes
  are indistinguishable from typing to a web UI. Verified live in Cursor.
- **Follow-up**: any future UIA-tier bug report should first ask "is the
  target web-rendered?".

### I-11 (v1.2) · Shared CSS bundle painted the transparent overlay window

- **Symptom**: a dark box behind the overlay pill on a `transparent: true`
  window; survived window-level fixes (`backgroundColor`, WebView2 env var).
- **Root cause**: `:global(body) { background: #141419 }` in
  Settings.svelte and Onboarding.svelte — all three windows share one
  compiled CSS bundle, and the later-in-bundle globals overrode the
  overlay's transparent body.
- **Why it happened**: per-window styling was done in component `<style>`
  blocks as if they were scoped; `:global` escapes that scope by definition.
  Debugging first assumed the compositor because the color read as "black".
- **Why the fix is correct**: the globals are deleted; non-overlay windows
  get their page background from the existing label check in `main.ts`,
  which runs only off-overlay. Verified live: pill floats with no box.
- **Follow-up**: rule — no `:global` visual styles in window components;
  window-specific page styling lives in `main.ts`'s label branch.

### I-7 (M9) · First e2e draft inserted mid-decode → text followed drifting focus

- **Symptom**: new end-to-end tests reported an empty EDIT control although
  every `insert` call returned Ok; landed text was nowhere to be seen.
- **Root cause**: the draft inserted each final *while* whisper was still
  decoding the rest of the fixture — seconds to minutes after the foreground
  check. By insert time the foreground had drifted, and the inserter did what
  it is designed to do (docs/02): follow the user's current focus.
- **Why it happened**: the harness pattern (I-6) verifies the foreground once
  before typing, which is safe only when typing follows immediately. Copying
  the pattern into a test with long decode gaps silently broke its premise.
- **Why the fix is correct**: the e2e now transcribes and cleans *everything
  first* (no window on screen), then creates the window and re-verifies
  foreground ownership + captured-process identity **before every insert**,
  skipping (never typing) if either check fails. The gap between verification
  and keystroke is back to milliseconds, and both tests pass with the exact
  cleaned transcript read back from the EDIT control.
- **Follow-up**: rule recorded here — any automated insertion must re-verify
  foreground *immediately* before each insert, not once per test.

### I-8 (M9) · Second app launch panicked: hotkey already registered

- **Symptom**: soak-test setup launched the release build while a debug
  instance was still alive; the new process panicked in the setup hook
  (`HotKey already registered`) after the fallback registration also failed.
- **Root cause**: global hotkeys are system-wide singletons; two instances
  can never coexist, but nothing enforced single-instance semantics.
- **Why it happened**: development always ran exactly one instance; the
  collision needed a second launch to surface, which real users hit daily
  (double-clicking the icon while the tray app is already running).
- **Why the fix is correct**: `tauri-plugin-single-instance` (registered
  first, before the shortcut plugin) hands the second launch off to the
  running instance, which opens its settings window — the expected tray-app
  behavior. The panic is unreachable because a second process never gets to
  the setup hook.
- **Follow-up**: none; covered by the plugin's own guarantees.

### I-1 (M1) · Float phase accumulator made the resampler chunk-variant

- **Symptom**: `resample_chunked_equals_whole` failed — same audio resampled
  in 487-sample chunks differed from one-shot resampling.
- **Root cause**: the fractional read position was re-anchored per chunk
  (`phase = pos - last_index`) in f64; the subtraction accumulates rounding
  error, so output sample positions depended on chunk boundaries.
- **Why it happened**: float phase is the "obvious" textbook implementation;
  the chunk-invariance requirement is specific to streaming use.
- **Why the fix is correct**: position is now a rational number
  (`pos_num / out_rate`) in u64 integer arithmetic. The sequence of positions
  is exactly the same regardless of chunking (subtraction of `end_num` is
  exact in integers), so outputs are bit-identical — proven by the same test.
  Overflow bound: `pos_num ≤ (chunk_len+1) × out_rate ≈ 2^33` for 1 s chunks,
  far under u64.
- **Follow-up**: none. Test guards regression.

### I-2 (M2) · whisper-rs 0.14 fails bindgen layout asserts on MSVC

- **Symptom**: `error[E0080]: attempt to compute 1_usize - 264_usize` in
  generated `bindings.rs` — a compile-time struct-size assertion.
- **Root cause**: whisper-rs-sys 0.13.1's build-time bindgen, driven by
  LLVM/clang 21, computes a `whisper_full_params` layout that disagrees with
  what rustc produces for the generated struct on x86_64-pc-windows-msvc.
- **Why it happened**: version skew between the crate's vintage and the
  locally installed libclang; layout asserts exist precisely to catch this.
- **Why the fix is correct**: whisper-rs 0.16 (sys 0.15) regenerates bindings
  compatibly with current clang; the layout asserts now pass, meaning the
  FFI structs are verified correct rather than assumed. API changes
  (`full_n_segments() -> i32`, segment accessor objects) were adapted in
  `od-stt`.
- **Follow-up**: `whisper-rs >= 0.16` noted in project memory; CI compiles
  from clean state so any future skew fails loudly in the same way.

### I-3 (M2) · Parallel test execution corrupted the latency measurement

- **Symptom**: finalize latency read 11 s in the default test run, 1.2 s when
  run with `--test-threads=1`.
- **Root cause**: three e2e tests each constructed a 4-thread whisper engine
  and decoded concurrently; 12+ compute threads on a laptop CPU starve each
  other, and the wall-clock metric measured contention, not the pipeline.
- **Why it happened**: cargo's default parallel test harness is wrong for
  benchmarking; fine for correctness.
- **Why the fix is correct**: perf-sensitive assertions documented to run
  serially (`--test-threads=1`, noted in the test header and project memory);
  the 3 s bound stays as a pathological-regression guard, and real
  measurement lives in this log, not in parallel CI runs.
- **Follow-up**: M9 soak/perf suite must pin serial execution for all
  latency measurements.

### I-5 (M4) · UIA SetValue parks the caret at 0 → later text prepends

- **Symptom**: harness test: two consecutive insertions came out as
  `"Ünïcode…Hello from Scribbet."` — second utterance *before* the first.
- **Root cause**: tier 1 (`ValuePattern::SetValue`) is WM_SETTEXT under the
  hood; it fills the field but leaves the caret at position 0, so the next
  utterance's SendInput events typed at the front.
- **Why it happened**: SetValue looks atomic and self-contained; its caret
  side effect only matters across *multiple* insertions in one session —
  exactly what dictation does and single-shot testing misses.
- **Why the fix is correct**: after a successful SetValue the inserter sends
  Ctrl+End (caret to end of document — correct for both single-line and
  multiline edits), so every later tier appends. Proven by the same harness
  test now asserting exact two-insert ordering.
- **Follow-up**: none for insertion; M6 voice-editing will manage the caret
  through UIA ranges explicitly.

### I-6 (M4) · Harness typed test strings into the developer's editor

- **Symptom**: during a harness run, `insert #2` text appeared in the
  developer's Cursor editor and its trailing newline submitted a chat
  message.
- **Root cause**: Windows denies `SetForegroundWindow` to background
  processes (foreground lock); the harness window never became foreground,
  and the inserter — following its by-design "follow the user's current
  focus" rule — typed into the real foreground app.
- **Why it happened**: the test assumed activation always succeeds; the
  first run happened to win the race, hiding the hazard.
- **Why the fix is correct**: the harness now (1) nudges the foreground lock
  (synthetic Alt tap) and retries activation, (2) *verifies*
  `GetForegroundWindow() == harness window` and skips the test entirely
  otherwise, and (3) asserts the captured focus process is the test binary
  before any insertion. A test that cannot own the foreground now refuses to
  type anywhere. Product behavior is unchanged — following the user's caret
  is the intended dictation semantic (docs/02); the hazard was purely in the
  test rig.
- **Follow-up**: any future example/demo binaries that insert must use the
  same guard pattern; noted for the M9 e2e suite.

### I-4 (M2) · Fixture pauses are longer than scripted

- **Symptom**: two tests assumed the `with_pause` fixture contains a 900 ms
  gap and `hello_world` is one utterance; VAD found ~1.8 s of silence and two
  utterances respectively.
- **Root cause**: SAPI TTS adds natural sentence-boundary silence on top of
  the scripted `AppendBreak(900 ms)`, and between sentences of a single
  prompt.
- **Why it happened**: assumption written before listening to/measuring the
  fixtures.
- **Why the fix is correct**: tests now assert against measured fixture
  behavior (bridge threshold 2.5 s; single-utterance assertions use the
  continuous `quick_fox` fixture) — the *system* behaved correctly throughout.
- **Follow-up**: none; fixture regeneration re-runs these tests.
