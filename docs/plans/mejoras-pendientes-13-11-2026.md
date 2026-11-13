# Mejoras y pendientes tras la revisión del 13-11-2026: plan de trabajo

Estado: en curso · 14-11-2026 (fase A cerrada; B1c en curso en `explore/b1c-cpu-fit`)

**Objetivo:** ordenar en un solo plan lo que queda de R3, los pendientes menores del README y las
propuestas de la [revisión de técnicas](../records/benchmarks/REVIEW_TECHNIQUES_13-11-2026.md),
según el [orden de trabajo acordado](../requirements.md#orden-de-trabajo-acordado): R3 → R5 (completado;
queda la robustez remota) → congelar V7 final → API pública. La velocidad de conversión (RQ-07) es
requisito para promover V7. Las prioridades activas siguen en el [README](../../README.md#prioridades-pendientes).

**Ramas:** `feat/r3-v75` (worktree `~/.cache/codex-worktrees/octree-r3-v75`, que guarda los datasets
v7.5 en `target/render-data`) recibe lo verificado. El trabajo sin verificar va en una rama `explore/`
y se integra con avance rápido cuando pasa su check. La revisión se escribió sobre `c473c85`; su
vigencia en esta rama se anota en cada tarea.

**Reglas:** cada tarea se cierra con su check en verde y `scripts/check.sh --offline`. Las
optimizaciones que pueden rechazarse (fase C) se integran solo con el visto bueno del propietario. Si
salta un criterio de parada de la Tarea 11, se consulta al propietario. Una subtarea cerrada se
borra al cerrar su tarea; el detalle queda en git y en los registros.

## Fase A: higiene y defectos baratos (cerrada)

- [x] A1. OQ-06 cerrada (ADR 0030) y plan indexado (`b50497f`).
- [x] A2. TW2: un `cacheBytes` distinto recrea el presupuesto de lectura en los dos workers (`43fc0d9`).
- [x] A3. Una página que falla no se pide en bucle y un injerto fallido no rompe la cola (`43fc0d9`).
- [x] A4. TA8: `#![forbid(unsafe_code)]` en los cinco crates sin `unsafe` (`5a1c7d8`).

## Fase B: terminar R3 ([plan](render-r3-cubos-en-gpu.md))

- [x] B1. Tarea 11 medida: estadísticas `cutApply` y `cellsForgottenTotal`, probe
  `probe-cells-smoke.mjs` y [registro](../records/benchmarks/R3_T11_SMOKE_13-11-2026.md) (`97c52f8`,
  `80a44bd`). Se asienta a 4 px; Majadas a 1 px dentro de una hoja entra en bucle de olvido (R3-Q27).
- [ ] B1c. Bucle de olvido de Majadas a 1 px ([ADR 0037](../adr/0037-corte-ajustado-al-presupuesto-de-cpu.md), enmendado por el [ADR 0038](../adr/0038-residencia-por-celdas.md)).
  Check final: el probe asienta las 4 poses de Majadas a 1 px sin olvidos en reposo, sin regresión a 4 px.
  - [x] B1c.1. Admisión por presupuesto del ADR 0037 (90/80, `cpuCoarsened`) (`721cac9`). No basta:
    `cpuCoarsened` sigue a 0 porque el corte solo pide 343 MB.
  - [x] B1c.2. Co-propietarios de bloques protegidos y contados (`721cac9`). No era la causa: 0 olvidados.
  - [x] B1c.3. Contadores de diagnóstico en el navegador (`84238ec`). A 120 s: solo se olvidan nodos
    seleccionados (6.498); 1.100 seleccionados, corte fijo en 32 y ~300 hijos construidos fuera del
    corte; caché de bloques llena (256 MiB) con 15.000 expulsiones.
  - [x] B1c.4. Test rojo en nativo `majadas_at_1px_settles_with_the_browser_block_cache` (`#[ignore]`,
    datos reales, caché de bloques de 256 MiB como el navegador): no se asienta en 6.000 frames; con
    2 GiB se asienta en 568. La causa es la residencia por bloques (`lod_transition.rs:157`): el corte
    a 1 px necesita ~518 MB de bloques (R3-Q28). Construir por grupo de hermanos quita las expulsiones
    pero choca con la caché llena de bloques protegidos: descartado.
  - [ ] B1c.5. Residencia por celdas (ADR 0038): en la ruta de celdas, un nodo con celdas es residente
    para el plan sin sus bloques, la caché no protege esos bloques y el corte se acota por bytes de
    celdas. Check: B1c.4 en verde y el resto de tests de celdas y de la ruta CPU.
  - [ ] B1c.6. Si el navegador sigue olvidando hijos de grupos incompletos tras B1c.5: protegerlos y
    contarlos en los bytes necesarios. Check: con un presupuesto menor que el conjunto de trabajo se
    asienta con `cpuCoarsened` > 0.
  - [ ] B1c.7. Caché de bloques: `blockEvictions` en el probe tras B1c.5; sin propuesta nueva si ya no satura.
  - [ ] B1c.8. Navegador: probe a 4 px y a 1 px (todas las poses), registro nuevo fechado y nota en la
    Tarea 11 del plan de R3.
  - [ ] B1c.9. Limpieza e integración: quitar los tests de diagnóstico `forget_loop_diagnosis` y
    `split_forget_loop_diagnosis` si ya no aportan, conservar los contadores baratos y la traza del
    probe, avance rápido de `feat/r3-v75`, `check.sh` y README.
- [ ] B1b. Casillas abiertas de la Tarea 16b.
  - [ ] B1b.1. Escala de la pirámide de `hzb.rs` (ruta CPU): test rojo con un viewport que no es
    potencia de dos (1280×720), corrección y profundidad idéntica con y sin oclusión.
  - [ ] B1b.2. Cota del margen de profundidad del Hi-Z (hoy 4 ulp empíricos): derivarla del error
    entre la caja proyectada y el rasterizador, y fijarla en un test en el límite.
  - [ ] B1b.3. ≤ 16,7 ms en wasm: el intervalo entre frames está atado al vsync (p95 17,1–17,9 ms sin
    frames perdidos). Medir el tiempo de GPU con `timestamp-query` en Chrome, o pedir al propietario
    que fije el criterio con el intervalo.
- [ ] B2. Tarea 12, modos de color, con ≤ 1 LSB en intensidad y elevación (R3-Q25).
  - [ ] B2.1. Mapear los modos de la ruta CPU y la vuelta a ella de R3-Q23 (qué lee cada modo y dónde).
  - [ ] B2.2. Color por atributo en la ruta de celdas: tests de paridad con la CPU, bit a bit en RGB y
    clasificación, ≤ 1 LSB en intensidad y elevación.
  - [ ] B2.3. Los modos de shader no reconstruyen celdas (test que cuenta construcciones).
  - [ ] B2.4. Las generaciones de color viejas se descartan (test).
  - [ ] B2.5. Quitar la vuelta a la ruta CPU de R3-Q23 y comprobarlo en el navegador.
- [ ] B3. Tarea 13: un mismo espaciado de referencia por nube (T0b) no cambia al injertar páginas.
- [ ] B4. Tarea 14, modos de px: fijo progresivo (predeterminado) y dinámico hacia 16,7 ms con histéresis (RF-06, RF-07).
- [ ] B5. TW6, pérdida de dispositivo: recuperar sin recargar (R3-Q26).
  - [ ] B5.1. El worker de render detecta la pérdida (callback de `wgpu`) y la expone en las estadísticas y en la API.
  - [ ] B5.2. Recrear adaptador, dispositivo, superficie y recursos; el pool usa `poolReset` y el
    worker de construcción reenvía las celdas; la cámara se conserva.
  - [ ] B5.3. Ruta CPU: volver a subir la escena.
  - [ ] B5.4. Playwright: caída del proceso GPU de Chrome (no `device.destroy()`, cuyo motivo
    `destroyed` no se recupera); la vista vuelve con el mismo digest y sin errores en consola.
- [ ] B6. Tarea 17, medida y ruta por defecto: matriz de R0 más RED-REF en el equipo de referencia
  (≥ 4 GB de GPU). Depende de B1c y B2; al cerrarla, actualizar el estado del ADR 0027.

## Fase C: velocidad de conversión, RQ-07 (rama `explore/convert-speed`)

Cada variante: mediana de 3 en Belvedere, SHA-256 de salida idéntico y V5 medido de nuevo en la
misma sesión para la razón v7.5/V5. No se mide a la vez que el navegador.

- [ ] C0. Línea base de la sesión: V5 y v7.5 en Belvedere, mediana de 3, con tiempos por fase.
- [ ] C1. TC3: `i128` fuera del bucle por punto en conversor y lector. Check adicional: paridad del lector y `decodeMs` en nativo y wasm.
- [ ] C2. TC2: acumulador de muestreo con datos calientes separados o agregación por lotes; `perf stat` de fallos de caché y TLB.
- [ ] C3. TC7: cola sin barrera por lote y `par_iter` efectivo. Check: curva 1–8 workers con RSS y orden de salida igual.
- Parada (umbral propuesto en este plan, no medido): si tras C1 y C2 Belvedere sigue en v7.5/V5 ≥ 1,7,
  se replantea RQ-07 alrededor de TC5.

## Fase D: fuente remota y V7 final

- [ ] D1. TW3 cancelación real con `AbortController` y TW5 reintentos con tiempo límite.
- [ ] D2. TW4 unión con huecos y TW10 concurrencia por fuente, medidas en RED-REF.
- [ ] D3. Congelar V7 final (ADR 0022), tras decidir OQ-09 (orden de filas, TF1).
- [ ] D4. TW8 `{url}` en la API pública; pertenece a la etapa de API pública, después de D3.

## Limpieza

- [x] L1. Copias sin commitear de la revisión en el checkout principal (`feat/render-r3-node-cells`),
  ya integradas en `b50497f`: descartadas el 13-11-2026.
- [ ] L2. Salidas temporales de los probes en `target/r3-t11/` (fuera de git): borrarlas al registrar B1c.8.
- [ ] L3. Rama `explore/b1c-cpu-fit`: borrarla tras integrarla en B1c.9.

## Decisiones del propietario pendientes

| Tema | Bloquea |
| --- | --- |
| OQ-09 orden de filas en bloque (TF1) | D3 |
| OQ-10 visor nativo o sin cabeza (TA4) | Planificar la API pública |
| Criterio de ≤ 16,7 ms si no se mide el tiempo de GPU (B1b.3) | B1b |
| ADR 0008 (`release` optimiza tamaño; usa `opt-level = 3` desde el 02-11-2026) y ADR 0025 (aceptado; el codificador propio no existe) | Nada; requieren ADR nuevo o decisión |
| PR de `feat/r3-v75` a `master` | Integración |
