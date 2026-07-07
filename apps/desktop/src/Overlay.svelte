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
  const BASE_PX = 5;
  const MAX_PX = 26;
  let heights = $state(ENVELOPE.map(() => BASE_PX));

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
      // Hot-mic boost (6x, as the old meter) + per-bar jitter so the whole
      // set dances with the voice instead of filling left to right.
      const drive = Math.min(1, level * 6);
      heights = ENVELOPE.map(
        (env) =>
          BASE_PX +
          drive * env * (0.6 + 0.4 * Math.random()) * (MAX_PX - BASE_PX),
      );
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
    title="OpenDictate — click to talk"
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
    min-width: 56px;
    height: 14px;
    padding: 0 10px;
    border-radius: 12px;
    background: rgba(8, 8, 10, 0.96);
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
    min-width: 66px;
    height: 36px;
    border-radius: 18px;
  }
  .sliver {
    width: 30px;
    height: 4px;
    border-radius: 2px;
    background: rgba(255, 255, 255, 0.85);
    transition: width 120ms ease;
  }
  .pill:hover .sliver {
    width: 38px;
  }
  .wave {
    display: flex;
    align-items: center;
    gap: 3px;
    height: 28px;
  }
  .bar {
    width: 3px;
    border-radius: 2px;
    background: rgba(255, 255, 255, 0.92);
    transition: height 70ms ease-out;
  }
  .pill.busy .bar {
    background: rgba(255, 255, 255, 0.45);
  }
</style>
