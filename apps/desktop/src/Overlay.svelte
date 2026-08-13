<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { invoke } from "@tauri-apps/api/core";

  /** Mirror of od_core_types::AppEvent (serde tag = "type", snake_case).
   *  The overlay only reacts to state changes; text events are ignored by
   *  design (this is a presence/mic indicator, not a transcript view). */
  type AppEvent =
    | { type: "state_changed"; state: "idle" | "listening" | "finalizing" }
    | { type: string; [key: string]: unknown };

  let sessionState = $state<"idle" | "listening" | "finalizing">("idle");

  // Nine hairline bars, center-weighted; all of them ride the level.
  const ENVELOPE = [0.5, 0.65, 0.8, 0.92, 1, 0.92, 0.8, 0.65, 0.5];
  // Per-bar wave phase/speed: organic motion instead of random flicker.
  const PHASES = ENVELOPE.map((_, i) => i * 1.7);
  const BASE_PX = 4;
  const MAX_PX = 16;
  let heights = $state(ENVELOPE.map(() => BASE_PX));
  let smoothed = ENVELOPE.map(() => BASE_PX);

  function onEvent(ev: AppEvent) {
    if (ev.type === "state_changed") {
      sessionState = (ev as Extract<AppEvent, { state: string }>).state as
        | "idle"
        | "listening"
        | "finalizing";
    }
  }

  function clickToTalk() {
    invoke("toggle_session");
  }

  onMount(() => {
    const unlisten = listen<AppEvent>("app-event", (e) => onEvent(e.payload));
    const poll = setInterval(async () => {
      const level =
        sessionState === "listening" ? await invoke<number>("get_level") : 0;
      // Square-root drive: normal speech RMS is small, and a linear map
      // left only the tallest bar visibly moving. sqrt lifts quiet levels
      // so the whole set responds, while loud speech still saturates.
      const drive = Math.min(1, Math.sqrt(level * 8));
      const t = performance.now() / 1000;
      smoothed = smoothed.map((h, i) => {
        // Per-bar sway with different speeds — lively, never random.
        const sway = 0.65 + 0.35 * Math.sin(t * (5 + i * 0.9) + PHASES[i]);
        const target = BASE_PX + drive * ENVELOPE[i] * sway * (MAX_PX - BASE_PX);
        return h + (target - h) * 0.5;
      });
      heights = smoothed.slice();
    }, 66);
    return () => {
      clearInterval(poll);
      unlisten.then((f) => f());
    };
  });
</script>

<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
<div class="dock">
  <div
    class="pill"
    class:live={sessionState === "listening"}
    class:busy={sessionState === "finalizing"}
    onclick={clickToTalk}
    title="Scribbet — click to talk"
  >
    {#if sessionState === "idle"}
      <span class="sliver"></span>
    {:else}
      <span class="wave">
        {#each heights as h, i (i)}
          <span class="bar" style="height: {h}px"></span>
        {/each}
      </span>
    {/if}
  </div>
</div>

<style>
  :global(html, body) {
    background: transparent;
    margin: 0;
    height: 100%;
    overflow: hidden;
  }
  .dock {
    height: 100%;
    display: flex;
    align-items: flex-end;
    justify-content: center;
    padding-bottom: 2px;
    box-sizing: border-box;
  }
  .pill {
    display: flex;
    align-items: center;
    justify-content: center;
    min-width: 44px;
    height: 14px;
    padding: 0 8px;
    border-radius: 12px;
    background: rgba(8, 8, 10, 0.65);
    border: 1px solid rgba(255, 255, 255, 0.32);
    box-shadow: 0 2px 10px rgba(0, 0, 0, 0.5);
    cursor: pointer;
    transition:
      height 160ms ease,
      min-width 160ms ease,
      border-color 160ms;
  }
  .pill:hover {
    border-color: rgba(255, 255, 255, 0.55);
  }
  .pill.live,
  .pill.busy {
    min-width: 58px;
    height: 26px;
    border-radius: 13px;
  }
  .sliver {
    width: 24px;
    height: 4px;
    border-radius: 2px;
    background: rgba(244, 87, 10, 0.9);
    transition: width 120ms ease;
  }
  .pill:hover .sliver {
    width: 30px;
    background: #f4570a;
  }
  .wave {
    display: flex;
    align-items: center;
    gap: 3px;
    height: 18px;
  }
  .bar {
    width: 2px;
    border-radius: 1px;
    background: #f4570a;
    transition: height 90ms ease-out;
  }
  .pill.busy .bar {
    background: rgba(244, 87, 10, 0.45);
  }
</style>
