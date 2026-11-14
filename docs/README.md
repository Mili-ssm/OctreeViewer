# Índice de documentación

Punto de entrada del proyecto: [README](../README.md) (estado y prioridades activas). Marcas:
**vivo** = se mantiene con el código; **histórico** = registro fechado, no se edita ([política](../CONTRIBUTING.md#convenciones)).

## Requisitos

| Documento | Contenido | Marca |
| --- | --- | --- |
| [Requisitos](requirements.md) | Propósito, usuarios, escenarios de escala, requisitos EARS, restricciones, supuestos y preguntas abiertas. | vivo |

## Referencia

| Documento | Contenido | Marca |
| --- | --- | --- |
| [Inventario](reference/inventory.md) | Mapa de componentes, identidades de formato, herramientas, límites y candidatos. | vivo |
| [Formato V5](reference/format/FORMAT_V5.md) | Especificación del `.oct`/`.octattrs` predeterminado. | vivo |
| [V7.1-pre](reference/format/FORMAT_V7_PRE1.md), [V7.2-pre](reference/format/FORMAT_V7_PRE2.md), [V7.3-pre](reference/format/FORMAT_V7_PRE3.md), [V7.4-pre](reference/format/FORMAT_V7_PRE4.md), [V7.5-pre](reference/format/FORMAT_V7_PRE5.md) | Especificaciones de las revisiones V7 experimentales (v7.5: índice paginado, R5). | vivo |

## Planes y specs

Cada plan lleva en la línea 3 `Estado: activo | pendiente | en pausa | cerrado · dd-mm-aaaa`
(`grep -n '^Estado:' docs/plans/*.md` da el estado de todos). Los cerrados no se reescriben: el
trabajo nuevo va en un plan nuevo.

| Documento | Contenido | Estado |
| --- | --- | --- |
| [Plan V7](plans/format-v7-plan.md) | Contrato, recursos acotados, async/paralelismo, candidatos de formato y retirada de V5/V6. | activo |
| [Render y carga a escala](plans/render-y-carga-escala.md) | Fases R0–R5: arnés y línea base, mejoras de LOD y carga (R1), worker de construcción (R2) y fases decididas R3–R5 (ADR 0019–0023). | activo |
| [Mejoras y pendientes (13-11-2026)](plans/mejoras-pendientes-13-11-2026.md) | Orden de trabajo tras la revisión de técnicas: higiene, cierre de R3, velocidad de conversión y robustez remota. | en curso |
| [R3: cubos por nodo en GPU](plans/render-r3-cubos-en-gpu.md) | Celdas por nodo independientes de la cámara en un pool de GPU paginado (ADR 0027), spikes primero, modos de huecos y de px. | completado |
| [R3: anchura de región, layout, máscaras y flags](plans/r3-anchura-de-region.md) | Experimento: anchura de página 8–32, ubicación de flags y modelos de página; decidido en ADR 0032. | completado |
| [R4: pirámide densa y rejilla global](plans/render-r4-piramide-densa.md) | Revisión `v7.4-pre`: hojas pequeñas, espaciado por nodo, rejilla anclada en el CRS y CRS completo; spike de densidad primero. | completado |
| [R5: jerarquía paginada](plans/render-r5-jerarquia-paginada.md) | Revisión `v7.5-pre`: páginas por niveles desde anclas, k por página, lector perezoso local y HTTP Range; flags LAS y Extra Bytes. | completado |
| [Conversor web paralelo](plans/conversor-web-paralelo.md) | Spec: paridad byte a byte con el CLI, núcleo sin I/O y pool de workers hasta escala Majadas; siguiente hito M6. | en pausa |
| [Etapa 1](plans/conversor-web-paralelo-etapa1.md), [2](plans/conversor-web-paralelo-etapa2.md), [3](plans/conversor-web-paralelo-etapa3.md), [4](plans/conversor-web-paralelo-etapa4.md) del conversor web | S0/M0/S1; trabajos por nodo, S8, S9 y M1; S2, S10, M2 y M3a; M3b con zonas OPFS y retirada del conversor antiguo. | cerrado |

## Explicación

| Documento | Contenido | Marca |
| --- | --- | --- |
| [Resolución regional LOD](explanation/REGIONAL_VOXEL_LOD.md) | Contrato de celdas, fallback y regresiones del LOD Replacement. | vivo |
| [Resúmenes por nodo](explanation/NODE_ATTRIBUTE_SUMMARIES.md) | Contenedores experimentales de resúmenes y diccionario conjunto. | vivo |

## Registros

Benchmarks en [records/benchmarks](records/benchmarks/); sus cifras pertenecen a la revisión, perfil y equipo con que se midieron.

| Documento | Contenido | Marca |
| --- | --- | --- |
| [Evidencia V7](records/benchmarks/format-v7-evidence.md) | Evidencia por incremento del plan V7; datos en [data/](records/benchmarks/data/). | vivo (se amplía) |
| [V7.3-pre frente a V5](records/benchmarks/STORAGE_V7_V73_VS_V5_2026-10-28.md) | Conversión y lectura completas en Belvedere y Majadas, 3 rondas; disco menor, tiempo/RSS mayores. | fechado |
| [Referencia CLI web](records/benchmarks/WEB_CONVERTER_BASELINE_2026-10-29.md) | SHA de salida del CLI y tiempos Belvedere/Majadas (M0) para la paridad del conversor web. | fechado |
| [Puerta R7 de M1](records/benchmarks/M1_R7_2026-10-29.md) | Belvedere `convert` (4 workers) tras M1 frente a la base: `Recursive` y cola de listos, y divisiones 8 (regresión, diagnóstico y corrección). | fechado |
| [Referencias SHA R1](records/benchmarks/R1_REFERENCE_SHA_2026-10-29.md) | SHA del CLI tras la norma portátil (ADR 0016); sustituyen a los de M0, cuyos tiempos siguen vigentes. | fechado |
| [Referencia SHA v7.4-pre (R4-T3)](records/benchmarks/R4_V74_REFERENCE_SHA_02-11-2026.md) | SHA del primer fixture `v7.4-pre` del escritor de `viewer_format`, junto a las referencias R1. | fechado |
| [Referencia SHA v7.4-pre con origen en cuantos (R4-T4)](records/benchmarks/R4_V74_REFERENCE_SHA_2_02-11-2026.md) | SHA de `seed-sample-rev8*` tras guardar el origen de marco como `i64` cuantos; sustituye al anterior. | fechado |
| [Referencia SHA v7.4-pre con extensión de datos (R4)](records/benchmarks/R4_V74_REFERENCE_SHA_3_02-11-2026.md) | SHA de `seed-sample-rev8*` tras guardar el máximo cuantizado de los datos en el índice; sustituye al anterior. | fechado |
| [Referencia SHA de las semillas v7.5-pre (R5)](records/benchmarks/R5_V75_REFERENCE_SHA_02-11-2026.md) | SHA de `seed-sample-rev9` y de las semillas de páginas encadenadas (`fuzz_paged_reader`). | fechado |
| [Referencia SHA de las semillas v7.5-pre, pareja y ids por página (R5)](records/benchmarks/R5_V75_REFERENCE_SHA_02-11-2026_2.md) | SHA de las semillas regeneradas tras volver a `.oct` + `.octattrs` y a ids `(página, índice)`; sustituye a la primera. | fechado |
| [R3-T0: línea base CPU independiente](records/benchmarks/R3_T0_CPU_04-11-2026.md) | Nueve muestras en el entorno actual; no se mezclan con la captura parcial. | completado |
| [R3-T0: fixtures en el anfitrión web real](records/benchmarks/R3_T0_WEB_REAL_04-11-2026.md) | Seis aperturas; jerarquía diferida e injerto en v7.5, índice completo en v7.4. | completado |
| [R3-T0: fixtures y línea base CPU parcial](records/benchmarks/R3_T0_PARTIAL_04-11-2026.md) | Fixtures en ambas versiones y tres muestras CPU en el entorno anterior. | parcial |
| [Referencia SHA de las semillas v7.5-pre con espaciado de referencia (R3-T0b)](records/benchmarks/R5_V75_REFERENCE_SHA_03-11-2026.md) | SHA de las semillas regeneradas al añadir el espaciado de referencia al bloque de metadatos; sustituye a la segunda. | fechado |
| [S2: rangos de chunks LAZ](records/benchmarks/LAZ_CHUNK_RANGES_2026-10-29.md) | Decodificación por rangos de chunks sin `seek`, fixture de chunks variables y velocidad en wasm32 frente a nativo. | fechado |
| [S10: RAM del híbrido](records/benchmarks/HYBRID_SPOOL_RAM_2026-10-29.md) | Pico de RAM y handles de spools híbridos RAM/OPFS; propuesta P3. | fechado |
| [Zonas OPFS (S5 residual)](records/benchmarks/OPFS_ZONES_2026-10-30.md) | Ficheros > 4 GiB, `truncate`, liberación de handles tras `terminate()` y RAM de JS en Chrome y Firefox; base del ADR 0017. | fechado |
| [Política del almacén híbrido](records/benchmarks/HYBRID_POLICY_2026-10-30.md) | Modelo de volcado y granularidad de segmentos y páginas sobre las trazas de S10. | fechado |
| [Puerta Node de M3b](records/benchmarks/M3B_NODE_GATE_2026-10-30.md) | Belvedere a través de los bindings en Node frente a `native_hybrid` y R7 corto de las pistas. | fechado |
| [M3b en navegadores](records/benchmarks/M3B_BELVEDERE_2026-10-30.md) | Belvedere en Chrome y Firefox: SHA, tiempo frente al nativo, PSS y OPFS con spools de 2 y 4 GiB. | fechado |
| [Línea base de render](records/benchmarks/RENDER_BASELINE_2026-10-30.md) | Arnés de órbita en Chrome y primer A/B de las mejoras R1 en Majadas y Belvedere. | fechado |
| [Etapa retenida al mover (R1-T1a)](records/benchmarks/RENDER_R1_T1A_01-11-2026.md) | Regresiones de etapa/prioridad y humo Majadas con worker y respaldo. | fechado |
| [Prioridad sobre pendientes (R1-T3b)](records/benchmarks/RENDER_R1_T3B_01-11-2026.md) | Coste de proyección, equivalencia de orden y humo Majadas. | fechado |
| [Medida después de R1 (R1-T7)](records/benchmarks/RENDER_R1_T7_01-11-2026.md) | Matriz R0 tras R1, comparación con R2-T5 y contraste de expulsiones con revisiones anteriores. | fechado |
| [Edad de escena tras R1 (R1-T7)](records/benchmarks/RENDER_R1_T7_SCENE_AGE_01-11-2026.md) | Por qué sube `sceneAgeMs` desde T1a y por qué se resuelve en R3. | fechado |
| [R3-T1: alternativas de profundidad](records/benchmarks/R3_T1_OPTIONS_04-11-2026.md) | Diagnóstico de 16 hojas filtradas y propuesta sujeta a aprobación. | propuesta |
| [R3-T1: repetición del benchmark en Majadas](records/benchmarks/R3_T1_BENCH_04-11-2026.md) | Salidas completas y confirmación del criterio de parada. | parada |
| [R3-T1: parada por profundidad en Majadas](records/benchmarks/R3_T1_STOP_04-11-2026.md) | Colisiones entre posiciones `f32` distintas y duplicados en el límite de profundidad. | parada |
| [R3: experimento CPU de celdas adaptativas](records/benchmarks/R3_ADAPTIVE_EXPERIMENT_04-11-2026.md) | V5 entre ramas y celdas v7.5 con continuación adaptativa; Belvedere falla por 22 bits. | fechado |
| [R3: rango de hojas](records/benchmarks/R3_LEAF_SPAN_04-11-2026.md) | Tres hojas de Belvedere necesitan 22 bits relativos; ninguna en Majadas. | fechado |
| [R3: subregiones experimentales](records/benchmarks/R3_SUBREGIONS_04-11-2026.md) | Conteos y tiempos nativos de una ejecución con clave por subregiones. | fechado |
| [R3: scratch compacto de una región](records/benchmarks/R3_LAYOUT_COMPACT_05-11-2026.md) | Registro de 48 B cuando el nodo cabe en una región; comparación nativa emparejada. | fechado |
| [R3-T5: spike de GPU en la Radeon 880M](records/benchmarks/R3_GPU_SPIKE_05-11-2026.md) | Primer tiempo de GPU: pool real con 12 B, 16 B y 40 B, *overdraw* y celdas visibles; salta el criterio de parada. | parada |
| [R3-T7d: coste de CPU del pool y tamaños exactos](records/benchmarks/R3_T7D_CPU_06-11-2026.md) | Plan de residencia ≤ 1,38 ms por frame a 4 px (antes 3,28) y sin recorte falso; Majadas a 1 px en movimiento, 2,76 ms. | completado |
| [R3-T9: `LodDataset` con celdas en nativo](records/benchmarks/R3_T9_CELLS_06-11-2026.md) | 9 casos asentados sin huecos ni retención; celdas ≤ 303 MiB de CPU en Majadas a 4 px; construcción de un nodo hasta 69 ms en p95. | completado |
| [R3-T10: celdas entre workers en el navegador](records/benchmarks/R3_T10_WORKERS_07-11-2026.md) | Mismo `cutDigestAtRest` con worker de construcción y sin él; frame p95 18,1 ms frente a 17,3 de la ruta CPU; sin worker, 100 ms (R3-Q24). | completado |
| [R3-T11: humo en navegador de la ruta de celdas](records/benchmarks/R3_T11_SMOKE_13-11-2026.md) | Alcanza el objetivo a 4 px en Belvedere y Majadas; Majadas a 1 px dentro de una hoja entra en bucle de olvido con el presupuesto de CPU de wasm lleno (R3-Q27). | fechado |
| [R3-B1c: bucle de olvido a 1 px](records/benchmarks/R3_B1C_CPU_FIT_14-11-2026.md) | Residencia por celdas, hijos en curso protegidos y bloqueo sin oscilación: Majadas a 1 px se asienta; Belvedere a 1 px tras un salto desborda el pool de GPU. | fechado |
| [R3-B1d: pool de 512 MiB](records/benchmarks/R3_B1D_GPU_POOL_14-11-2026.md) | Pool de 65.535 páginas con presupuesto propio y corte ajustado al pool: se asientan las 14 poses a 1 y 4 px; GTT pico ~+750 MiB. | fechado |
| [R3-B1b: Hi-Z y tiempo de GPU](records/benchmarks/R3_B1B_HIZ_14-11-2026.md) | Fallo de escala de `hzb.rs` corregido, margen del Hi-Z derivado (+3 % de GPU) y tiempo de GPU en wasm: p95 7,38 ms en Majadas dentro de una hoja a 4 px. | fechado |
| [R3-B2: modos de color en celdas](records/benchmarks/R3_B2_COLOR_14-11-2026.md) | Todos los modos con celdas y función de color común (paridad con diferencia 0); modos de shader sin reconstruir; atributos con generación de color. | fechado |
| [R3-B2b: intensidad y clase en la ruta asíncrona](records/benchmarks/R3_B2B_ASYNC_ATTRS_14-11-2026.md) | Corrige B2: el worker de construcción no leía intensidad ni clase en modo RGB; ahora intensidad y clasificación coinciden con la ruta CPU en el navegador. | fechado |
| [R3-B3: continuidad por nube](records/benchmarks/R3_B3_CONTINUITY_14-11-2026.md) | Mínimo de cubo de 1 × el espaciado de referencia, sin reconstruir al alternar; efecto visual mínimo en Belvedere y Majadas. | fechado |
| [R3-B4: modos de px](records/benchmarks/R3_B4_PX_MODES_14-11-2026.md) | Fijo progresivo por defecto y dinámico 1–8 px hacia 16,7 ms con el tiempo de GPU; en Chrome refina y engrosa según la carga. | fechado |
| [R3-B5: pérdida de dispositivo](records/benchmarks/R3_B5_DEVICE_LOST_14-11-2026.md) | Recuperación sin recargar en 0,61 s tras una caída del proceso GPU de Chrome, con la misma pose y el mismo corte; canvas sustituido por la API. | fechado |
| [R3-T17: medida de la ruta de celdas](records/benchmarks/R3_T17_MEASURE_14-11-2026.md) | Celdas frente a CPU, local y RED-REF: RQ-01 y RQ-05 pasan; RQ-02 salvo +2,7 pp de píxeles vacíos en Majadas. | fechado |
| [Velocidad de conversión, fase C (exploración)](records/benchmarks/EXPLORE_CONVERT_SPEED_14-11-2026.md) | v7.5/V5 sigue en ~1,9: la diferencia está en el muestreo de la jerarquía; TC3, TC1 y TC2(b) no bastan. | fechado |
| [R3-T7c: medida nativa del pool de celdas](records/benchmarks/R3_T7C_NATIVE_06-11-2026.md) | `kpc` de producción en 3,28 ms en el peor caso (spike: 4,39 ms); el plan de residencia cuesta hasta 8,75 ms de CPU y la estimación de tamaños recorta de más. | parada |
| [R3-T6: aceptación de la instancia de 16 B](records/benchmarks/R3_T6_ACCEPT_05-11-2026.md) | Páginas idénticas al spike en 22 casos, decodificación bit a bit en la Radeon 880M (Vulkan y GL) y en llvmpipe con `fma()` explícito y origen relativo al ojo (≤ 0,0006 px). | completado |
| [R3-T16a: compactación y Hi-Z sobre el pool](records/benchmarks/R3_T16_OCCLUSION_05-11-2026.md) | Compactación en GPU con Hi-Z por celda en dos fases: Majadas dentro de una hoja a 4 px de 21,66 a 4,39 ms, sin agujeros; el Hi-Z por página no aporta. | completado |
| [R3-T15: aceptación del nivel por octante](records/benchmarks/R3_T15_ACCEPT_05-11-2026.md) | 0 diferencias por octante entre `cell_cut` y el oráculo del spike; rangos de dibujo por corte. | completado |
| [R3-T15: bytes residentes con nivel por octante](records/benchmarks/R3_T15_RESIDENT_05-11-2026.md) | Niveles completos residentes; `octant_tight` cabe en 256 MiB (238,9 MiB en el peor caso). | fechado |
| [R3: opciones para no dibujar celdas ocultas](records/benchmarks/R3_OCCLUSION_OPTIONS_05-11-2026.md) | HZB en dos pasadas, raymarching, híbrido o puntos por cómputo; estado actual y qué medir (OQ-08). | evaluación |
| [R3-T4: celdas por encima del objetivo](records/benchmarks/R3_T4_OFFAXIS_05-11-2026.md) | Protección dentro del octante y factor fuera de eje; el factor global hace fallar Majadas, las demás variantes pasan. | fechado |
| [R3-T4: alternativas al error proyectado](records/benchmarks/R3_T4_METRIC_05-11-2026.md) | Métrica ajustada y nivel por octante frente a la cota; `octant_tight` casi iguala a la CPU salvo 2,27× a 1 px. | fechado |
| [R3-T4: spike de cobertura nativo](records/benchmarks/R3_T4_COVERAGE_05-11-2026.md) | Instancias, bytes y `DescendantTooFine` por pose en Belvedere y Majadas; saltan los criterios de parada. | parada |
| [R3-T1: aceptación de celdas por nodo](records/benchmarks/R3_T1_ACCEPT_05-11-2026.md) | Ninguna hoja sin separar en Belvedere y Majadas completos con la continuación adaptativa. | fechado |
| [R3: layout de páginas por anchura de región](records/benchmarks/R3_REGION_LAYOUT_05-11-2026.md) | Anchuras 8–32 con flags por página o por instancia sobre Belvedere y Majadas completos; gana W = 10 con instancia de 12 B. | fechado |
| [Celdas por nodo sobre datos reales (R3-T1)](records/benchmarks/RENDER_R3_T1_NODE_CELLS_01-11-2026.md) | Longitud y bytes de la cadena de hoja en Majadas y Belvedere; criterios de parada de R3. | fechado |
| [Spike de densidad de la pirámide (R4-T1)](records/benchmarks/R4_DENSITY_SPIKE_02-11-2026.md) | Hoja × `sample_grid` en Majadas y Belvedere: salto de espaciado, disco, RSS y tiempo. | fechado |
| [Memoria del conversor con g = 256 (R4-T1)](records/benchmarks/R4_SPARSE_GRID_RSS_02-11-2026.md) | Causa y corrección de la RSS (umbral `mmap` de glibc, rejilla dispersa compacta). | fechado |
| [Tiempo del conversor con g = 256 (R4-T1)](records/benchmarks/R4_CONVERT_TIME_02-11-2026.md) | Escaneo raíz en paralelo y ventana deslizante de bloques. | fechado |
| [Tiempo del conversor, segunda ronda (R4-T1)](records/benchmarks/R4_CONVERT_TIME_2_02-11-2026.md) | Celda de muestreo en una pasada y tabla dispersa compacta; Majadas +24 % frente a V5. | fechado |
| [Conversión v7.4-pre frente a V5 (R4-T4b)](records/benchmarks/R4_T4B_CONVERSION_02-11-2026.md) | Tiempo, RSS y disco de Majadas y Belvedere; tiempo +92–115 %. | fechado |
| [Matriz de render y R3-T1 sobre v7.4-pre (R4-T6)](records/benchmarks/R4_T6_MEASUREMENT_02-11-2026.md) | Regresión de píxeles vacíos en Belvedere; contadores anómalos; hojas en el tope 1,4–2,0 %. | fechado |
| [Matriz de render tras los arreglos del visor (R4-T6b)](records/benchmarks/R4_T6B_MEASUREMENT_02-11-2026.md) | Pasa los criterios de render; píxeles vacíos de Belvedere 0,71–0,76 → 0,29–0,36; R3 sigue en pausa. | fechado |
| [opt-level 3 frente a "s" (exploración)](records/benchmarks/EXPLORE_OPT_LEVEL_02-11-2026.md) | Conversor nativo −20 % con opt-level 3; no cambia la proporción v7.4/V5. | fechado |
| [opt-level 3 en el WASM (exploración)](records/benchmarks/EXPLORE_OPT_LEVEL_WASM_02-11-2026.md) | Descarga +7–8 % comprimida; edad de escena −14 %; tiempo al objetivo −18 %. | fechado |
| [Hojas en el tope de R3-T1 (exploración)](records/benchmarks/EXPLORE_ADAPTIVE_LEAF_02-11-2026.md) | Meseta de espaciado con 8 celdas y bench según ADR 0028: 0,06 % y 0,14 % en el tope. | fechado |
| [Tamaño de página de la jerarquía paginada (R5-P0)](records/benchmarks/R5_P0_PAGES_02-11-2026.md) | k = 5 vale en aéreo (p95 16–27 KB), no con ramificación 8; índice −50 %. | fechado |
| [Bytes de la cadena de hoja (exploración)](records/benchmarks/EXPLORE_LEAF_CHAIN_BYTES_02-11-2026.md) | Poda de niveles casi duplicados: 0 hojas > 4 ×, cadena ≈ 1 × puntos. | fechado |
| [CPU por nivel del conversor (exploración)](records/benchmarks/EXPLORE_CONVERT_CPU_02-11-2026.md) | Copia de registros y muestreo por ranura: −12–15 %, salida idéntica; v7.5/V5 sigue ~2×. | fechado |
| [Revisión de técnicas 13-11-2026](records/benchmarks/REVIEW_TECHNIQUES_13-11-2026.md) | Contraste del conversor, la carga web, el render, el formato y la arquitectura con técnicas publicadas: propuestas `TC`, `TW`, `TR`, `TF` y `TA` con su medida de aceptación; sin implementar ni medir. | fechado |
| [Apertura remota por HTTP Range (R5-P4)](records/benchmarks/R5_P4_HTTP_RANGE_02-11-2026.md) | RED-REF: primer frame 370–390 ms y 4 px ≤ 1,3 s con v7.5-pre; RQ-04 pasa; v7.4 como base. | fechado |
| [Worker de construcción (R2)](records/benchmarks/RENDER_BUILD_WORKER_2026-10-30.md) | Escenas construidas fuera del hilo de render, activado y no: respuestas de frame y tiempo a objetivo en Majadas y Belvedere. | fechado |
| [Digest del worker de construcción](records/benchmarks/RENDER_BUILD_WORKER_DIGEST_2026-10-30.md) | Digest en reposo distinto entre ambas rutas: la causa es la cámara de la sonda, no el worker. | fechado |
| [Diagnóstico de T4](records/benchmarks/RENDER_T4_DIAGNOSIS_2026-10-30.md) | Causa de la regresión de T4 en Belvedere: orden de carga de los bloques pendientes. | fechado |
| [Puerta R7 de M2](records/benchmarks/M2_R7_2026-10-29.md) | Instrucciones (callgrind) y R7 corto en divisiones 8 tras extraer `viewer_convert_core`. | fechado |
| [Paridad M3a](records/benchmarks/M3A_PARITY_2026-10-29.md) | Motor `core` frente al CLI en Chrome, Firefox y Node; hallazgo de `hypot`. | fechado |
| [Spike OPFS (arenas)](records/benchmarks/OPFS_ARENA_SPIKE_2026-10-29.md) | Modelo de pico de arenas frente al nativo y MB/s de OPFS en Chrome/Firefox (HTTP y `file://`). | fechado |
| [Modelo de arenas por nodo](records/benchmarks/NODE_JOB_ARENA_MODEL_2026-10-29.md) | Pico modelado de los trabajos por nodo frente al nativo (variantes a, b, c) y fórmula para M6. | fechado |
| [Cuota OPFS](records/benchmarks/OPFS_QUOTA_2026-10-29.md) | Cuota real de OPFS y `persist()` en Chrome y Firefox, y si cabe Majadas. | fechado |
| [Spike OPFS (un fichero por spool)](records/benchmarks/OPFS_SPOOL_FILES_2026-10-29.md) | Ficheros vivos, handles síncronos concurrentes y coste por fichero en Chrome/Firefox. | fechado |
| [Web](records/benchmarks/WEB_PERFORMANCE_BENCH.md) | Métricas de navegador, flujos y limitaciones visuales. | histórico |
| [Memoria v5](records/benchmarks/V5_MEMORY_BENCH.md) | Conversión, acceso a bloques, memoria y perfiles nativos. | histórico |
| [Vóxeles](records/benchmarks/VOXEL_BUILD_BENCH.md) | Construcción, memoria conjunta y reutilización de celdas. | histórico |
| [Almacenamiento](records/benchmarks/STORAGE_BENCH.md) | Suite repetida: tamaños, escritura, reconstrucción, carga de hijos. | histórico |
| [Preparación v6](records/benchmarks/STORAGE_V6_PREPARATION.md) | Traits, compatibilidad y criterios de v6. | histórico |
| [Pre3](records/benchmarks/STORAGE_V6_PRE3_BENCH.md), [pre4](records/benchmarks/STORAGE_V6_PRE4_BENCH.md), [SensOFOREST](records/benchmarks/SENSOFOREST_V6_BENCH.md) | Parejas válidas, tiempos, lectura selectiva y RSS. | histórico |
| [I/O v6](records/benchmarks/STORAGE_V6_IO.md), [CPU](records/benchmarks/STORAGE_V6_CPU.md), [planos de bytes](records/benchmarks/STORAGE_V6_BYTE_PLANES.md) | Pipeline optativo, CRC/diccionarios y localidad (tiling descartado). | histórico |
| [Diccionarios](records/benchmarks/ATTRIBUTE_DICTIONARY_BENCH.md) | Experimento de diccionarios de atributos. | histórico |
| [Características nightly](records/benchmarks/nightly-features-2026-10-28.md) | Evaluación de `portable_simd` en CRC32 y planos de bytes (ninguna adoptada). | fechado |
| [Ocupación cuantizada](records/benchmarks/QUANTIZED_OCCUPANCY_BENCH.md) | Residuales exactos y coste del índice. | histórico |
| [Selección LOD](records/benchmarks/LOD_FRONTIER_BENCH.md) | Frontera y prioridad incremental. | histórico |
| [Datos reales](records/benchmarks/REAL_DATASET_BENCH.md) | Protocolo y resultados con un escaneo grande. | histórico |
| [Ramificación espacial](records/benchmarks/SPATIAL_BRANCHING_BENCH.md), [anchura `u16`](records/benchmarks/COORDINATE_WIDTH_BENCH.md) | Experimentos descartados o acotados. | histórico |
| [V6 pre1](records/archive/FORMAT_V6_PRE1.md), [pre2](records/archive/FORMAT_V6_PRE2.md), [pre3](records/archive/FORMAT_V6_PRE3.md), [pre4](records/archive/FORMAT_V6_PRE4.md), [V4](records/archive/FORMAT_V4.md) | Especificaciones de formatos retirados o en retirada. | histórico |
| [Entregas 14-10-2026](records/archive/DELIVERIES_2026-10-14.md) | Implementaciones y experimentos cerrados, con checks históricos. | histórico |
| [Evaluación 24-10-2026](records/archive/CODE_ASSESSMENT_2026-10-24.md) | Contraste de documentación, checks y próximos pasos. | histórico |
| [Evaluación de render 30-10-2026](records/archive/RENDER_ASSESSMENT_2026-10-30.md) | Carga y render a escala (cientos de GB, 1–8 px): mapa, hallazgos B1–B7, investigación externa y mejoras Q1–Q8. | histórico |
| [Evaluación 28-10-2026](records/archive/CODE_ASSESSMENT_2026-10-28.md) | Revisión por áreas, plan de 15 tareas, decisiones, verificación y pendientes. | histórico |
| [Dependencias del renderer](records/archive/RENDERER_DEPENDENCIES.md) | Migración a wgpu 30/glam 0.34 (14-10-2026), checks y validación parcial de navegador; decisión en el ADR 0010. | histórico |
| [Historial técnico](records/archive/DEVELOPMENT_HISTORY.md) | Snapshot anterior a v5. | histórico |

## Decisiones

[ADR](adr/README.md): decisiones de formato, versiones, límites, toolchain, seguridad y conversión web, con alternativas y condiciones de revisión (vivo).

## Datos y recursos

[Datasets de escala](../data/datasets/README.md) y [muestras](../data/samples/README.md) (procedencia y preparación); [host web](../examples/web-host/README.md) (API, build, probes; en inglés); [capturas](point-cloud-previews/) de nubes de ejemplo.

## Dónde va cada cosa

| Contenido nuevo | Destino | Regla |
| --- | --- | --- |
| Requisito, restricción, supuesto o pregunta abierta para el propietario | [requirements.md](requirements.md) | IDs estables (`RF-`, `RQ-`, `CON-`, `ASM-`, `OQ-`); no se renumeran |
| Decisión de diseño duradera | [adr/](adr/README.md) | `Estado:` en la línea 3; una aceptada se sustituye o enmienda con un ADR nuevo |
| Spec o plan de implementación | [plans/](plans/) | `Estado:` en la línea 3; enlazarlo en este índice |
| Contrato vigente (formato, API, límites) | [reference/](reference/) | Se actualiza en el mismo cambio que el código |
| Por qué funciona algo así, sin pasos | [explanation/](explanation/) | Vivo; los resultados fechados van a registros |
| Medida, spike, evaluación o diagnóstico | [records/benchmarks/](records/benchmarks/) | `NOMBRE_dd-mm-aaaa.md`, inmutable; perfil, equipo y revisión dentro |
| Documento retirado o sustituido | [records/archive/](records/archive/) | Inmutable salvo enlaces rotos |
| Estado y prioridades activas | [README](../README.md#prioridades-pendientes) | Solo allí |
