import { createSignal, createEffect } from "solid-js";

export type Theme = "geek-dark" | "classic-dark" | "light";

export const [currentTheme, setCurrentTheme] = createSignal<Theme>("geek-dark");
export const [currentZoom, setCurrentZoom] = createSignal<number>(100);

// Set theme class on the document root when it changes
createEffect(() => {
  const t = currentTheme();
  document.documentElement.className = t;
});

// Set zoom style on the document root when it changes
createEffect(() => {
  const z = currentZoom();
  // Using zoom property for Chromium webview engine
  document.documentElement.style.zoom = (z / 100).toString();
});
