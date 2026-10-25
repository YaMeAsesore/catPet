#!/usr/bin/env node
// Contraste WCAG del sistema visual «Mac 1-bit».
// Uso: node scripts/wcag.mjs  ·  Sin dependencias. Salida no-cero si algún
// par exigible (4.5 texto / 3 componentes) baja del mínimo.
const hex = (h) => {
  const s = h.replace("#", "");
  return [0, 2, 4].map((i) => parseInt(s.slice(i, i + 2), 16) / 255);
};
const lum = (rgb) => {
  const f = (c) => (c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4));
  const [r, g, b] = rgb.map(f);
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
};
const ratio = (a, b) => {
  const [l1, l2] = [lum(hex(a)), lum(hex(b))].sort((x, y) => y - x);
  return (l1 + 0.05) / (l2 + 0.05);
};

// [nombre, tinta, fondo, mínimo exigido]
const pairs = [
  ["CLARO  texto / papel", "#14130F", "#F2EFE3", 4.5],
  ["CLARO  secundario / papel", "#55524A", "#F2EFE3", 4.5],
  ["CLARO  texto / elevado", "#14130F", "#E9E5D5", 4.5],
  ["CLARO  secundario / elevado", "#55524A", "#E9E5D5", 4.5],
  ["CLARO  papel / tinta (selección y botón principal)", "#F2EFE3", "#14130F", 4.5],
  ["CLARO  texto hover botón (papel / tinta-hover)", "#F2EFE3", "#2A2823", 4.5],
  ["CLARO  borde tinta / papel (componente)", "#14130F", "#F2EFE3", 3],
  ["CLARO  foco tinta / papel (componente)", "#14130F", "#F2EFE3", 3],
  ["CLARO  separador / papel (decorativo)", "#C9C4B2", "#F2EFE3", 1],
  // M3.2: hover de fila por inversión (papel sobre tinta) — el par que pinta
  // `body.onebit .row:hover` para texto, artista, duración y acciones.
  ["CLARO  hover fila (papel / tinta)", "#F2EFE3", "#14130F", 4.5],
  ["OSCURO texto / fondo", "#EDE8DA", "#131210", 4.5],
  ["OSCURO secundario / fondo", "#A9A492", "#131210", 4.5],
  ["OSCURO texto / elevado", "#EDE8DA", "#1D1B18", 4.5],
  ["OSCURO secundario / elevado", "#A9A492", "#1D1B18", 4.5],
  ["OSCURO fondo / tinta (selección y botón principal)", "#131210", "#EDE8DA", 4.5],
  ["OSCURO texto hover botón (fondo / tinta-hover)", "#131210", "#FBF8EF", 4.5],
  ["OSCURO borde tinta / fondo (componente)", "#EDE8DA", "#131210", 3],
  ["OSCURO foco tinta / fondo (componente)", "#EDE8DA", "#131210", 3],
  ["OSCURO separador / fondo (decorativo)", "#3A3730", "#131210", 1],
  // M3.2: hover de fila por inversión (fondo sobre tinta clara).
  ["OSCURO hover fila (fondo / tinta)", "#131210", "#EDE8DA", 4.5],
  // M4: estados nuevos de los diálogos 1-bit, todos por inversión tinta/papel
  // (sección activa de Ajustes, chip CSV «no encontrada» y checkbox marcado).
  ["CLARO  sección activa de Ajustes (papel / tinta)", "#F2EFE3", "#14130F", 4.5],
  ["CLARO  chip CSV «no encontrada» (papel / tinta)", "#F2EFE3", "#14130F", 4.5],
  ["CLARO  checkbox marcado (tinta / papel, componente)", "#14130F", "#F2EFE3", 3],
  ["OSCURO sección activa de Ajustes (fondo / tinta clara)", "#131210", "#EDE8DA", 4.5],
  ["OSCURO chip CSV «no encontrada» (fondo / tinta clara)", "#131210", "#EDE8DA", 4.5],
  ["OSCURO checkbox marcado (tinta clara / fondo, componente)", "#EDE8DA", "#131210", 3],
];

let fail = 0;
for (const [name, fg, bg, min] of pairs) {
  const r = ratio(fg, bg);
  const ok = r >= min;
  if (!ok) fail++;
  console.log(`${ok ? "OK   " : "FALLA"} ${name}: ${r.toFixed(2)}:1 (mín ${min})`);
}
process.exit(fail ? 1 : 0);
