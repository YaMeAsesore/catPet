import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";

// Decisión: archivo presente porque svelte-check lo usa para resolver el preprocesado.
export default { preprocess: vitePreprocess() };
