import "./styles.css";
import App from "./App.svelte";
import { mount } from "svelte";

// Decisión: mount (Svelte 5) porque el MVP no necesita router.
const app = mount(App, { target: document.getElementById("app")! });
export default app;
