import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Decisión: puerto fijo 1420 porque tauri.conf.json devUrl lo espera.
export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { target: "es2021" },
});
