# Nivel pedido, LOD por celda y tamaño mínimo del cubo

Estado: en curso · 15-11-2026 · aprobado por el propietario (pre-mortem aplicado)

**Objetivo:** que el render dibuje en cada celda el nivel que pide su distancia, construyendo a demanda los niveles
que la cadena podada no guarda, con transiciones graduales en bandas alrededor del ojo, y que el usuario elija el
tamaño mínimo del cubo en px y en longitud (m/cm/mm o el espaciado de la nube) a la vez, ganando el mayor en cada celda.

**Decisiones de partida:** [ADR 0048](../adr/0048-el-render-dibuja-el-nivel-pedido.md) (#35),
[ADR 0050](../adr/0050-nivel-por-celda-segun-distancia.md) (#44) y [ADR 0049](../adr/0049-tamano-minimo-del-voxel.md).
El tamaño de hoja ([ADR 0051](../adr/0051-granularidad-de-hojas-configurable.md), #46) va después y no entra aquí.

**Evidencia (15-11-2026, lectura de código):** las celdas se dibujan con un único `drawIndexedIndirect`; un paso de
cómputo por instancia (`slot_state`, `cell_render.rs`) ya descarta por octante, máscara, frustum y Hi-Z; las páginas
están relativas al ojo en f32 y empiezan en la primera celda de un tramo Morton, no en un borde alineado. La
residencia es todo o nada por (nodo, nivel); el worker de construcción hace 2 nodos por actualización con todos sus
niveles. El presupuesto real es el pool de 512 MiB ([ADR 0040](../adr/0040-pool-de-512-mib-y-corte-ajustado-al-pool.md)) con umbral de ajuste del 90 %; no hay
RQ de tiempo de GPU. Nadie lee la unidad del CRS.

## Diseño (decisiones técnicas del agente)

| # | Decisión | Por qué |
| --- | --- | --- |
| D1 | El nivel pedido es cualquier profundidad; uno que falta se pide al worker de construcción (`levelRequests` → `nodeLevels`), que lo construye desde los bloques del nodo con la agregación de `build_level_keyed`. Implementado en la fase 1 así: la petición (`levelRequests{gen, nodes, depths}`) es el conjunto completo en orden de prioridad y sustituye al anterior; `gen` descarta los conjuntos viejos (`cutEpoch` y la generación de color se descartaron por no hacer falta); un fallo de caché programa la lectura de bloques; las peticiones tienen presupuesto propio, aparte de las 2 construcciones por actualización; los niveles a demanda cuentan en el presupuesto de CPU (los de nodos del corte no esperan al freno de admisión) y se reconstruyen con el color nuevo al recolorear; las peticiones pendientes sobreviven al recoloreo | ADR 0048; sin cancelación ni prioridad las peticiones se acumulan durante un zoom |
| D2 | Selección por celda en `slot_state`, **sobre coordenadas enteras relativas al nodo**: la página lleva su origen relativo al nodo (u32×3) y cada nodo un ancla f32 relativa al ojo. El ancestro es `(rel_página + rel) >> (d − d_prev)`. Una sola función WGSL `refines(nodo, prof, coord)` sirve para la celda y para su madre, con un gemelo Rust bit a bit para picking, métricas y dorados. La prueba va antes de la separación de fases de oclusión, para que las dos fases usen el mismo predicado | Sin coordenadas enteras, madre e hija redondean distinto: dobles cubos o huecos |
| D3 | Regla monótona: `refina(c) := lado(c) > max(lado_mín, px_objetivo · max(dist(punto de c más cercano al ojo, ojo), cerca) / focal)`. El uniforme del frame gana objetivo de px, focal y lado mínimo; la página gana los niveles residentes vecinos (la entrada probablemente pasa de 32 a 48 B) | Sin monotonía, un nivel intermedio y uno fino se dibujan a la vez |
| D4 | Residencia acotada (aprobada por el propietario, 16-11-2026, tras la fase 0a): cada nodo mantiene las profundidades que dibuja más la siguiente más fina (R2: 294 MiB en Majadas a 1 px dentro de una hoja, frente a 531 MiB con el rango contiguo). Mantener siempre también la más gruesa (R3c) da 522 MiB a 1 px, por encima de 512, así que se mantiene **solo si cabe** (aceptado por el propietario, 16-11-2026): es lo primero que se suelta con presión de memoria. Al alejarse sin el nivel grueso residente, los hijos no se sueltan hasta que el grueso está listo: se construye mientras tanto (a demanda, D1) o se regenera antes de soltar los hijos, así que nunca queda un hueco ([registro](../records/benchmarks/R3_CELL_LOD_0A_15-11-2026.md)). La distancia cercana se limita al plano cercano. Si el corte no cabe, sube el objetivo de px efectivo de toda la nube ([ADR 0052](../adr/0052-retencion-y-ajuste-al-pool-por-celda.md), R3-Q11) | Desde dentro de un nodo, el rango contiguo superaba los 512 MiB; la nube debe verse homogénea |
| D8 | Opcional (ADR 0052, revisión): si la población de los hijos ya está disponible sin coste (nivel hijo construido pero aún no en GPU), la celda que debe refinar puede dibujarse como sus hijos poblados con el color del padre; si no, se mantiene hasta que cargan. No se añade nada a la instancia solo para esto | Idea oportunista del propietario, no requisito |
| D5 | Celdas que abarcan octantes: cuentan como «refina» si la máscara del nodo no está vacía; una celda más gruesa que la rejilla 8³ prueba todos los bits de máscara que cubre | Hoy se saltan octante y máscara y pintarían encima de hijos residentes |
| D6 | Unidad del CRS: `viewer_format` lee solo la `UNIT` de primer nivel del PROJCS o sistema de coordenadas (WKT1, WKT2, compuesto) y la clave GeoTIFF 3076; un CRS geográfico desactiva el modo longitud; sin unidad, metros | ADR 0049; un `UNIT["degree"]` anidado no debe desactivar una nube proyectada |
| D7 | La UI sustituye la casilla de continuidad por dos filtros que actúan a la vez: el mínimo en px (el objetivo de siempre) y el mínimo en longitud (desactivado, m/cm/mm o espaciado de la nube); en cada celda gana el mayor (propietario, 16-11-2026). `setContinuity` queda para el espaciado | ADR 0049, revisión del 16-11-2026 |

## Fases

- [x] **0a. Medida en CPU antes de tocar la GPU** (15-11-2026, [registro](../records/benchmarks/R3_CELL_LOD_0A_15-11-2026.md); salta 512 MiB a 1 px con el rango contiguo, resuelto con la nueva D4). Ampliar `examples/coverage_spike.rs` con la regla D2/D3 y la
  residencia D4. Majadas y Belvedere en órbita, rasante y dentro de una hoja, a 1, 2, 4 y 8 px, en «solo px» y
  «espaciado». Registrar instancias dibujadas, bytes residentes (con niveles a demanda bajo el espaciado),
  profundidad máxima pedida y peticiones por segundo de zoom. Comprobación: registro en `docs/records/benchmarks/`.
- [x] **0b. Pregunta al propietario:** R3-Q19, R3-Q21 y R3-Q11 por celda. Resuelta en el
  [ADR 0052](../adr/0052-retencion-y-ajuste-al-pool-por-celda.md) (15-11-2026).
- [ ] **1. Niveles a demanda (ADR 0048, #35)** ([plan detallado](fase-1-niveles-a-demanda.md); implementado y verificado el 16-11-2026 salvo el p95 de petición a residente, que pasa a la fase 2 ([#54](https://github.com/Mili-ssm/OctreeViewer/issues/54)); [registro](../records/benchmarks/R3_FASE1_NIVELES_A_DEMANDA_16-11-2026.md)). D1. El corte y el pool dejan de suponer «deseado ∈ guardados» y ya
  envían un rango de profundidades y una prioridad. Comprobación: `fs_e430` (#35) se dibuja como superficie en la
  demo; tests del protocolo y del corte; p95 de petición a residente medido; dorados regenerados con su motivo;
  `scripts/check.sh` completo.
- [ ] **2a. Selección por celda tras una opción `cellLod: octant|cell` (predeterminada `octant`).** D2, D3 y D5 (D8 solo si sale gratis) sobre
  niveles residentes, con el gemelo Rust, el picking y una prueba de partición en GPU (patrón de
  `cell_render_tests.rs`): leer `visible` y comprobar una sola celda dibujada por punto, con ≥3 niveles, profundidades
  no contiguas, un nivel que abarca octantes con máscara, oclusión activada y desactivada, ≥1000 poses aleatorias con
  bordes de banda y ojo dentro de una celda. Contadores de GPU (instancias por profundidad, celdas por encima del
  objetivo) leídos en `perf`; la señal «en reposo» pasa a un resumen del gemelo; se actualizan
  `probe-cells-parity`, `PROBES.md` y `REGIONAL_VOXEL_LOD.md`.
- [ ] **2b. Residencia D4 y transiciones de un nivel ([ADR 0053](../adr/0053-transiciones-de-un-nivel.md), RF-31).** Conjunto de niveles por nodo contiguo entre el más grueso y el más fino dibujado (intermedios a demanda), como mucho un nivel por frame al acercarse y al alejarse, evaluar la derivación de niveles gruesos desde los hijos residentes (sin releer bloques), y medida de la cola bajo la separación (pendiente de revisión).
- [ ] **2c. Activar `cell` por defecto tras medir.** Comprobación según el ADR 0050: Playwright acercándose a la
  muestra (bandas), y ningún octante **ni nodo** cambia entero en un frame, medido con una diferencia de frames
  cuando entra un nodo hijo, y ninguna celda cambia más de un nivel entre frames ni difiere en más de uno de sus vecinas (ADR 0053); si la máscara por nodo no lo permite, abrir una pregunta antes de seguir.
- [ ] **3. Selector de tamaño mínimo (ADR 0049).** D6 y D7. Comprobación: tests de la unidad del CRS (metro, pie,
  grados, compuesto pie/metro, sin unidad); Playwright de los dos filtros combinados (longitud desactivada, en m/cm/mm y por espaciado) en escritorio y móvil, con teclado; sonda de
  continuidad actualizada.
- [ ] **4. Cierre.** Quitar la opción `cellLod`, `docs/estado.md`, ADR 0031/0034 si cambian cifras, demo republicada,
  #35 y #44 cerradas. Comprobación: la demo pública abre la muestra y una nube convertida pequeña con bandas y sin
  cortes.

Cada fase de código tendrá su plan detallado (`writing-plans`) al empezarla.

## Riesgo abierto tras la fase 0a

- Con la regla D3 (distancia y focal) entre el 1,6 % y el 13 % de las celdas dibujadas quedan por encima del objetivo
  de px, y medir al punto más cercano no lo corrige; `octant_tight` quedaba en ≤ 0,26 % (ADR 0034). La fase 2a debe
  usar la arista proyectada real (`projected_voxel_edge_pixels`) o justificar otra regla monótona que cumpla RQ-09.

## Criterios de parada

- Fase 0a: bytes residentes por encima de 460 MiB (90 % de 512) a ≥ 2 px, o de 512 MiB a 1 px, en cualquier pose de
  Majadas → replantear D4 con el propietario.
- Con D2 sobre enteros, la prueba de partición sigue encontrando doble cobertura o huecos → pasar a la rejilla de
  512 celdas por nodo (alternativa del ADR 0034).
- Instancias por encima de 2,5× la ruta CPU a 1 px o de 2× a 2–8 px en la pose rasante (ADR 0034) → parar la fase 2.
- La cola de peticiones crece sin parar en un zoom de 10 s, o el p95 de petición a residente supera 1 s a 4 px →
  parar D1 y añadir cancelación primero.
- El p95 de RQ-01 llega a 33 ms durante un zoom en el equipo de referencia → no activar `cell` por defecto.
- La fase 2a pasa de unos 5 días de trabajo → reajustar el alcance.
