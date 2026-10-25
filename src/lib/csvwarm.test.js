// S4.2b.1: tests puros del calentado (node:test, sin dependencias).
// Ejecutar: npm test   (o: node --test src/lib/)
import { test } from "node:test";
import assert from "node:assert/strict";
import { runWarm } from "./csvwarm.ts";

/** Ganchos por defecto: todo vivo, nada tocado, todos ok, backoff cede. */
const hooks = (over = {}) => ({
  alive: () => true,
  touched: () => false,
  attempt: async () => "ok",
  waitThrottle: async () => true,
  ...over,
});

test("S4.2b.1: un fallo en la fila 3 no corta; el calentado llega hasta la 7", async () => {
  const intentos = [];
  const out = await runWarm(8, hooks({
    attempt: async (i) => {
      intentos.push(i);
      return i === 3 ? "fail" : "ok";
    },
  }));
  assert.deepEqual(out, [0, 1, 2, 3, 4, 5, 6, 7]);
  assert.ok(out.includes(7), "sigue más allá del fallo de la fila 3");
  assert.deepEqual(intentos, [0, 1, 2, 3, 4, 5, 6, 7]);
});

test("el backoff pausa y reintenta la MISMA fila, y después sigue", async () => {
  let esperas = 0;
  let vistos1 = 0;
  const out = await runWarm(3, hooks({
    attempt: async (i) => {
      if (i === 1) {
        vistos1 += 1;
        if (vistos1 === 1) return "throttle";
      }
      return "ok";
    },
    waitThrottle: async () => {
      esperas += 1;
      return true;
    },
  }));
  assert.deepEqual(out, [0, 1, 1, 2]);
  assert.equal(esperas, 1);
});

test("cancelar la espera del backoff detiene el calentado", async () => {
  const intentos = [];
  const out = await runWarm(4, hooks({
    attempt: async (i) => {
      intentos.push(i);
      return i === 1 ? "throttle" : "ok";
    },
    waitThrottle: async () => false,
  }));
  assert.deepEqual(out, [0, 1]);
  assert.deepEqual(intentos, [0, 1]);
});

test("las filas ya tocadas se saltan y una vista muerta corta el bucle", async () => {
  let viva = true;
  const intentos = [];
  const out = await runWarm(6, hooks({
    alive: () => viva,
    touched: (i) => i === 1 || i === 3,
    attempt: async (i) => {
      intentos.push(i);
      if (i === 4) viva = false; // la vista se cierra durante el calentado
      return "ok";
    },
  }));
  assert.deepEqual(out, [0, 2, 4]); // ni tocadas ni posteriores al cierre
  assert.deepEqual(intentos, [0, 2, 4]);
});
