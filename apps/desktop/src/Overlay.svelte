<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { invoke } from "@tauri-apps/api/core";

  /** Mirror of od_core_types::AppEvent (serde tag = "type", snake_case). */
  type AppEvent =
    | { type: "state_changed"; state: "idle" | "listening" | "finalizing" }
    | { type: "speech_started"; at_ms: number }
    | { type: "speech_ended"; at_ms: number }
    | {
        type: "partial_updated";
        segment_id: number;
        text: string;
        stable_len: number;
      }
    | { type: "final_ready"; segment_id: number; raw: string };

  let state = $state<"idle" | "listening" | "finalizing">("idle");
  let partial = $state("");
  let finals = $state<string[]>([]);
  let level = $state(0);

  const display = $derived(
    [...finals, partial].filter((t) => t.length > 0).join(" "),
  );

  function onEvent(ev: AppEvent) {
    switch (ev.type) {
      case "state_changed":
        state = ev.state;
        if (ev.state === "listening") {
          partial = "";
          finals = [];
        }
        if (ev.state === "idle") {
          partial = "";
        }
        break;
      case "partial_updated":
        partial = ev.text;
        break;
      case "final_ready":
        finals = [...finals, ev.raw];
        partial = "";
        break;
    }
  }

  onMount(() => {
    const unlisten = listen<AppEvent>("app-event", (e) => onEvent(e.payload));
    const poll = setInterval(async () => {
      level = state === "listening" ? await invoke<number>("get_level") : 0;
    }, 66);
    return () => {
      clearInterval(poll);
      unlisten.then((f) => f());
    };
  });

  // 12 bars, filled proportionally to the (RMS) level with a hot-mic boost
  // so normal speech visibly moves the meter.
  const bars = Array.from({ length: 12 }, (_, i) => i);
  const lit = $derived(Math.round(Math.min(1, level * 6) * bars.length));
</script>

<div class="pill" class:listening={state === "listening"}>
  <span
    class="dot"
    class:live={state === "listening"}
    class:busy={state === "finalizing"}
  ></span>
  <div class="meter">
    {#each bars as b (b)}
      <span class="bar" class:on={b < lit}></span>
    {/each}
  </div>
  <div class="text" class:placeholder={display.length === 0}>
    {#if display.length > 0}
      {#each finals as f, i (i)}<span class="final">{f} </span>{/each}{#if partial}<span
          class="partial">{partial}</span
        >{/if}
    {:else if state === "listening"}
      Listening…
    {:else if state === "finalizing"}
      Finishing…
    {:else}
      OpenDictate
    {/if}
  </div>
</div>

<style>
  .pill {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 56px;
    margin: 12px;
    padding: 0 18px;
    border-radius: 28px;
    background: rgba(24, 24, 30, 0.92);
    border: 1px solid rgba(255, 255, 255, 0.09);
    box-shadow: 0 8px 28px rgba(0, 0, 0, 0.45);
    color: #e8e8ee;
    font:
      500 14px/1.35 "Segoe UI Variable",
      "Segoe UI",
      system-ui,
      sans-serif;
  }
  .dot {
    flex: none;
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: #5a5a66;
    transition: background 120ms;
  }
  .dot.live {
    background: #3ddc84;
    box-shadow: 0 0 8px rgba(61, 220, 132, 0.7);
  }
  .dot.busy {
    background: #ffb02e;
  }
  .meter {
    flex: none;
    display: flex;
    align-items: center;
    gap: 2px;
    height: 20px;
  }
  .bar {
    width: 3px;
    height: 6px;
    border-radius: 2px;
    background: rgba(255, 255, 255, 0.14);
    transition: height 80ms, background 80ms;
  }
  .bar.on {
    height: 16px;
    background: #3ddc84;
  }
  .text {
    flex: 1;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    direction: rtl; /* keep the newest words visible when overflowing */
    text-align: left;
  }
  .text > :global(*) {
    direction: ltr;
    unicode-bidi: embed;
  }
  .text.placeholder {
    color: #8a8a96;
    direction: ltr;
  }
  .partial {
    color: #9a9aa8;
    font-style: italic;
  }
</style>
