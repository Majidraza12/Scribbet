import { mount } from "svelte";
import Overlay from "./Overlay.svelte";

// M3 ships one UI surface: the overlay pill. The settings window (M7) will
// route on the Tauri window label here.
const app = mount(Overlay, {
  target: document.getElementById("app")!,
});

export default app;
