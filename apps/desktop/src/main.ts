import { mount } from "svelte";
import { getCurrentWindow } from "@tauri-apps/api/window";
import Overlay from "./Overlay.svelte";
import Settings from "./Settings.svelte";
import Onboarding from "./Onboarding.svelte";

// One bundle, three windows: route on the Tauri window label (overlay pill,
// M7 settings window, M8 onboarding).
const label = getCurrentWindow().label;
const component =
  label === "settings" ? Settings : label === "onboarding" ? Onboarding : Overlay;
const app = mount(component, {
  target: document.getElementById("app")!,
});

if (label !== "overlay") {
  // The shared index.html is transparent for the overlay's sake; the
  // other windows want a solid page.
  document.documentElement.style.background = "#141419";
  document.body.style.overflow = "auto";
}

export default app;
