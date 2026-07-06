import { mount } from "svelte";
import { getCurrentWindow } from "@tauri-apps/api/window";
import Overlay from "./Overlay.svelte";
import Settings from "./Settings.svelte";

// One bundle, two windows: route on the Tauri window label (overlay pill vs
// the M7 settings window).
const label = getCurrentWindow().label;
const app = mount(label === "settings" ? Settings : Overlay, {
  target: document.getElementById("app")!,
});

if (label === "settings") {
  // The shared index.html is transparent for the overlay's sake; the
  // settings window wants a solid page.
  document.documentElement.style.background = "#141419";
  document.body.style.overflow = "auto";
}

export default app;
