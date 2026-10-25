// S4.2b.1: control PURO del calentado inicial (sin red ni UI): el llamador
// inyecta los ganchos reales y aquí queda la secuencia, testeable con
// `npm test` (node:test, sin dependencias).

export type WarmAttempt = "ok" | "fail" | "throttle";

export interface WarmHooks {
  /** ¿La vista de origen sigue viva? */
  alive: () => boolean;
  /** ¿La fila ya fue tocada (cualquier estado distinto de «sin emparejar»)? */
  touched: (i: number) => boolean;
  /**
   * Intenta calentar una fila:
   * - "ok": hay candidato (o negativo cacheado).
   * - "fail": error sin backoff de límite → se SALTA la fila.
   * - "throttle": backoff de límite activo → PAUSA y reintenta la misma.
   */
  attempt: (i: number) => Promise<WarmAttempt>;
  /** Espera a que pase el backoff; false = cancelar el calentado. */
  waitThrottle: () => Promise<boolean>;
}

/**
 * Bucle del calentado inicial (≤n primeras filas). S4.2b.1:
 * - un fallo NO corta el calentado: se salta la fila y sigue con la siguiente;
 * - si el backoff de límite se activa, PAUSA y reintenta la MISMA fila al
 *   reanudar (no lo cancela).
 * Devuelve los índices intentados (los repetidos por backoff, incluidos).
 */
export async function runWarm(n: number, h: WarmHooks): Promise<number[]> {
  const intentos: number[] = [];
  for (let i = 0; i < n; i++) {
    if (!h.alive()) break;
    if (h.touched(i)) continue; // ya la tocó el usuario (o el barrido)
    let r = await h.attempt(i);
    intentos.push(i);
    while (r === "throttle") {
      if (!(await h.waitThrottle())) return intentos;
      if (!h.alive()) return intentos;
      r = await h.attempt(i);
      intentos.push(i);
    }
    // "ok" y "fail" siguen con la siguiente fila (S4.2b.1).
  }
  return intentos;
}
