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
| P1-1 | STT finalization latency: `end_utterance` re-decodes the whole utterance | ~1.2 s for a 2.3 s utterance (release, base.en Q5, 4 threads) | Speech-end → final text ≤ 300 ms p50 / ≤ 500 ms p95 on the fixture set, measured by `whisper_e2e` serial run on the dev machine. Candidate fix: at SpeechEnd decode only audio after the last cadence decode, splice with the stable prefix; fall back to full re-decode when agreement is empty. | M2 |

(New P1 items get a row here; nothing may be silently dropped from this table
— items leave it only by meeting their acceptance criteria or by an explicit
user decision recorded in this file.)

## Performance baselines per milestone

Numbers from the dev machine (4-core+ laptop-class CPU, no GPU assumed).
"n/a" = the surface doesn't exist yet at that milestone.

| Metric | M1 | M2 | M3 | Target (v1) |
|---|---|---|---|---|
| Cold start (process → hotkey live) | n/a | n/a | **523 ms** (debug build, eager model load) | ≤ 2 s |
| Idle RAM | n/a | n/a | **129 MB** WS (debug; whisper model resident ≈ 59 MB of that) | ≤ 120 MB (250 hard) |
| Idle CPU | n/a | n/a | **0 %** over 10 s sample | ≤ 5 % |
| STT partial cadence | n/a | 700 ms configured | unchanged | partial visible ≤ 500 ms behind voice |
| STT finalize (speech-end → final) | n/a | ~1200 ms (P1-1) | unchanged (P1-1 open) | ≤ 300 ms p50 |
| Hotkey → listening | n/a | n/a | **51 ms** (toggle → mic open + Listening published) | ≤ 100 ms |
| Ring overruns during capture | 0 (2 s live) | 0 (fixtures) | 0 (live session) | 0 |
| Transcript accuracy on fixtures | n/a | exact on 4/4 | unchanged | exact |

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

## Issue log

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
  `"Ünïcode…Hello from OpenDictate."` — second utterance *before* the first.
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
