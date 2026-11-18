# Muestra de la demo en V7

Estado: completado · 15-11-2026

**Objetivo:** la muestra integrada de la demo usa la última versión estable del formato (V7) y es más densa,
para que enseñe la ruta de celdas real; V5 queda como formato heredado de comparación.

**Decisiones del propietario (15-11-2026):** la nube de ejemplo usa la última versión estabilizada; V5 es un
legado comparativo que se elimina cuando empiece un V8; la muestra debe ser mucho más granular
(768×768 = 589.824 puntos, 4,2 MiB; 1024² pesaba 8,4 MiB, demasiado para abrir la demo).

**Evidencia de partida:** `examples/web-host/sample.oct` es V5 (4.096 puntos, paso 1,0) y se dibuja por la ruta
CPU (`supports_cells()` exige marco global). `FormatWriter` no escribe formatos de marco global: V7 solo sale
del conversor.

## Tareas

- [x] Generador y muestra V7: ejemplo de `viewer_convert` que escribe la superficie en LAS y la convierte con las
  opciones por defecto (V7); la muestra V5 pasa a `sample-v5.oct` (legado) con su generador renombrado.
  Comprobación: `cargo test -p viewer_format --test web_sample` y regenerar dos veces da los mismos bytes.
- [x] Decisión y documentación: ADR 0047 (V5 heredado hasta V8; precisa el ADR 0004), README de `web-host`,
  índices. Comprobación: checks de docs y enlaces de `scripts/check.sh`.
- [x] Verificación y publicación: `scripts/check.sh` completo; Playwright sobre la demo local (la muestra abre
  por la ruta de celdas, escritorio y móvil); PR, fusión y `scripts/publish-demo.sh --push`.
  Comprobación: la demo pública abre la muestra V7.
