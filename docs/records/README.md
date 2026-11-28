# Índice de registros

Medidas, spikes, evaluaciones y documentos retirados. Los registros son inmutables y este índice es el único fichero de `docs/records/` que se edita ([reglas](../../CONTRIBUTING.md#convenciones); nombres de fichero en [«Dónde va cada cosa»](../README.md#dónde-va-cada-cosa)). Las cifras de benchmark pertenecen a la revisión, perfil y equipo con que se midieron.

Fuentes: [benchmarks/](benchmarks/) (medidas) y [archive/](archive/) (documentos retirados y planes cerrados).

## Formato y semillas de referencia

| Documento | Contenido | Marca |
| --- | --- | --- |
| [Evidencia V7](benchmarks/format-v7-evidence.md) | Evidencia por incremento del plan V7; datos en [data/](benchmarks/data/). | histórico (V7; la evidencia nueva va en registros fechados) |
| [V7.3-pre frente a V5](benchmarks/STORAGE_V7_V73_VS_V5_2026-10-28.md) | Conversión y lectura completas en Belvedere y Majadas, 3 rondas; disco menor, tiempo/RSS mayores. | fechado |
| [Referencias SHA R1](benchmarks/R1_REFERENCE_SHA_2026-10-29.md) | SHA del CLI tras la norma portátil (ADR 0016); sustituyen a los de M0, cuyos tiempos siguen vigentes. | fechado |
| [Referencia SHA v7.4-pre (R4-T3)](benchmarks/R4_V74_REFERENCE_SHA_02-11-2026.md) | SHA del primer fixture `v7.4-pre` del escritor de `viewer_format`, junto a las referencias R1. | fechado |
| [Referencia SHA v7.4-pre con origen en cuantos (R4-T4)](benchmarks/R4_V74_REFERENCE_SHA_2_02-11-2026.md) | SHA de `seed-sample-rev8*` tras guardar el origen de marco como `i64` cuantos; sustituye al anterior. | fechado |
| [Referencia SHA v7.4-pre con extensión de datos (R4)](benchmarks/R4_V74_REFERENCE_SHA_3_02-11-2026.md) | SHA de `seed-sample-rev8*` tras guardar el máximo cuantizado de los datos en el índice; sustituye al anterior. | fechado |
| [Referencia SHA de las semillas v7.5-pre (R5)](benchmarks/R5_V75_REFERENCE_SHA_02-11-2026.md) | SHA de `seed-sample-rev9` y de las semillas de páginas encadenadas (`fuzz_paged_reader`). | fechado |
| [Referencia SHA de las semillas v7.5-pre, pareja y ids por página (R5)](benchmarks/R5_V75_REFERENCE_SHA_02-11-2026_2.md) | SHA de las semillas regeneradas tras volver a `.oct` + `.octattrs` y a ids `(página, índice)`; sustituye a la primera. | fechado |
| [Referencia SHA de las semillas v7.5-pre con espaciado de referencia (R3-T0b)](benchmarks/R5_V75_REFERENCE_SHA_03-11-2026.md) | SHA de las semillas regeneradas al añadir el espaciado de referencia al bloque de metadatos; sustituye a la segunda. | fechado |
| [OQ-09: orden Morton en los bloques](benchmarks/OQ09_MORTON_ORDER_14-11-2026.md) | Orden híbrido: `.oct` −35 % sin más bytes por vista; todo Morton carga un 16–24 % antes pero descarga un 34–49 % más. Decidido en el ADR 0041. | fechado |
| [D3a: orden de los puntos y construcción de celdas](benchmarks/D3A_CELL_SORT_ORDER_14-11-2026.md) | El constructor ordenaba con pdqsort, sensible al orden de entrada; con un radix estable construir las celdas cuesta un 13–46 % menos con cualquier fichero. | fechado |
| [D3b: escritor híbrido de orden de filas](benchmarks/D3B_HYBRID_WRITER_15-11-2026.md) | `--row-order hybrid`: Belvedere 815 → 531 MB con un +1,3 % de tiempo total de conversión; Majadas apenas cambia. | fechado |
| [D3c-T7: referencias, datasets y semillas con el V7 final](benchmarks/D3C_V7_REFERENCES_15-11-2026.md) | SHA por defecto (V7) de los casos de R1 y de los datasets de render; Belvedere coincide con el SHA web; semillas existentes intactas (+`rev10`); tests con datos reales y la app alcanzan el objetivo. | fechado |
| [Formato v8.1-pre frente a v7](benchmarks/FORMATO_V8_PRE1_20-11-2026.md) | Mismo binario, tres nubes: salida −40,6 % (Albuera, V2 cumplido), −40,5 % (Majadas) y −34,8 % (Belvedere), conversión −3 % / −5 % / +4 %, cuantiles de distancia y carga en el navegador de Belvedere (mediana 798 ms frente a 918 ms, 4 ejecuciones). | fechado |
| [Formato v8.2-pre: esquemas de compresión de `.octattrs`](benchmarks/FORMATO_V8_PRE2_20-11-2026.md) | Cuatro esquemas zstd frente a LZ4: salida −3,08 / −5,69 / −9,45 % en Albuera, conversión ≤ ~3 % más lenta, diferencia de carga `lz4`/`all` en el navegador no medible (80 ejecuciones). | fechado |
| [Equivalencia V1 de v8.1-pre frente a v7](benchmarks/V1_EQUIVALENCIA_V8_PRE1_21-11-2026.md) | Mismo binario, cuatro entradas (dos muestras, Belvedere, Majadas): precondición cumplida y 0 diferencias en hojas (todas las columnas, byte a byte, y color de pantalla) y en LOD; solo faltan las columnas que D2 retira. Comprobación de datos, no de píxeles. | fechado |
| [Tests ignorados sobre datos reales en V8](benchmarks/RENDER_DATA_V8_21-11-2026.md) | Belvedere y Majadas reconvertidos a V8: los tests ignorados de `viewer_wasm` dan los mismos valores que con v7; datos v7 antiguos idénticos a los nuevos. | fechado |
| [Tamaño de la salida v7](benchmarks/TAMANO_SALIDA_V7_20-11-2026.md) | De dónde salen los bytes de la salida: Albuera guarda 1,97 N puntos (LOD) y el color dos veces (RGB de pantalla derivable, 19 %); candidatos para una V8. | fechado |

## Conversión del CLI y temporales

| Documento | Contenido | Marca |
| --- | --- | --- |
| [Referencia CLI web](benchmarks/WEB_CONVERTER_BASELINE_2026-10-29.md) | SHA de salida del CLI y tiempos Belvedere/Majadas (M0) para la paridad del conversor web. | fechado |
| [Puerta R7 de M1](benchmarks/M1_R7_2026-10-29.md) | Belvedere `convert` (4 workers) tras M1 frente a la base: `Recursive` y cola de listos, y divisiones 8 (regresión, diagnóstico y corrección). | fechado |
| [Velocidad de conversión, fase C (exploración)](benchmarks/EXPLORE_CONVERT_SPEED_14-11-2026.md) | v7.5/V5 sigue en ~1,9: la diferencia está en el muestreo de la jerarquía; TC3, TC1 y TC2(b) no bastan. | fechado |
| [C4: copia diferida y razón v7.5/V5](benchmarks/C4_DEFERRED_COPY_15-11-2026.md) | La copia diferida no acelera la jerarquía (cota −1,8 %); la conversión v7.5 tarda ~2,0× V5, y el híbrido ocupa un 10,7 % menos que V5. | fechado |
| [D3c-T3: RQ-07 con el V7 final](benchmarks/D3C_RQ07_V7_15-11-2026.md) | V7 tarda ×2,015 y usa +35,8 % de RSS frente a V5 en Belvedere: pasa el umbral del ADR 0043 (≤ ×2,1, ≤ +40 %); el par ocupa un 10,7 % menos. | fechado |
| [D3c-T6b: conversión web de Belvedere con el V7 final](benchmarks/D3C_WEB_BELVEDERE_15-11-2026.md) | En Chrome, V7 tarda ×1,87 lo que V5 (141,9 frente a 75,9 s; cota ≤ ×2,1) y el PSS pico sube de 3,76 a 4,75 GiB; SHA idénticos a los del CLI nativo. | fechado |
| [Temporales del CLI, fase B](benchmarks/TEMPORALES_CLI_FASE_B_20-11-2026.md) | Spools comprimidos (planos de bytes + LZ4): pico aparente de temporales ÷ 3,2 (Albuera, 11,1 × → 3,5 × el LAZ), ÷ 5,1 (Majadas) y ÷ 4,3 (Belvedere), salida idéntica, +7–24 % de tiempo. | fechado |
| [opt-level 3 frente a "s" (exploración)](benchmarks/EXPLORE_OPT_LEVEL_02-11-2026.md) | Conversor nativo −20 % con opt-level 3; no cambia la proporción v7.4/V5. | fechado |
| [opt-level 3 en el WASM (exploración)](benchmarks/EXPLORE_OPT_LEVEL_WASM_02-11-2026.md) | Descarga +7–8 % comprimida; edad de escena −14 %; tiempo al objetivo −18 %. | fechado |
| [CPU por nivel del conversor (exploración)](benchmarks/EXPLORE_CONVERT_CPU_02-11-2026.md) | Copia de registros y muestreo por ranura: −12–15 %, salida idéntica; v7.5/V5 sigue ~2×. | fechado |
| [Revisión de técnicas 13-11-2026](benchmarks/REVIEW_TECHNIQUES_13-11-2026.md) | Contraste del conversor, la carga web, el render, el formato y la arquitectura con técnicas publicadas: propuestas `TC`, `TW`, `TR`, `TF` y `TA` con su medida de aceptación; sin implementar ni medir. | fechado |
| [Puerta R7 de M2](benchmarks/M2_R7_2026-10-29.md) | Instrucciones (callgrind) y R7 corto en divisiones 8 tras extraer `viewer_convert_core`. | fechado |

## Almacenamiento web y OPFS

| Documento | Contenido | Marca |
| --- | --- | --- |
| [S2: rangos de chunks LAZ](benchmarks/LAZ_CHUNK_RANGES_2026-10-29.md) | Decodificación por rangos de chunks sin `seek`, fixture de chunks variables y velocidad en wasm32 frente a nativo. | fechado |
| [S10: RAM del híbrido](benchmarks/HYBRID_SPOOL_RAM_2026-10-29.md) | Pico de RAM y handles de spools híbridos RAM/OPFS; propuesta P3. | fechado |
| [Zonas OPFS (S5 residual)](benchmarks/OPFS_ZONES_2026-10-30.md) | Ficheros > 4 GiB, `truncate`, liberación de handles tras `terminate()` y RAM de JS en Chrome y Firefox; base del ADR 0017. | fechado |
| [Política del almacén híbrido](benchmarks/HYBRID_POLICY_2026-10-30.md) | Modelo de volcado y granularidad de segmentos y páginas sobre las trazas de S10. | fechado |
| [Puerta Node de M3b](benchmarks/M3B_NODE_GATE_2026-10-30.md) | Belvedere a través de los bindings en Node frente a `native_hybrid` y R7 corto de las pistas. | fechado |
| [M3b en navegadores](benchmarks/M3B_BELVEDERE_2026-10-30.md) | Belvedere en Chrome y Firefox: SHA, tiempo frente al nativo, PSS y OPFS con spools de 2 y 4 GiB. | fechado |
| [D1: cancelación y reintentos en HTTP Range](benchmarks/D1_REMOTE_CANCEL_RETRY_14-11-2026.md) | Reintentos con tiempo límite y descarga abortada al cancelar un bloque; sin efecto medible en Majadas por RED-REF. | fechado |
| [D2: unión con huecos y concurrencia remota](benchmarks/D2_GAP_CONCURRENCY_14-11-2026.md) | 8 lecturas remotas por defecto (−15 a −30 % hasta el objetivo en la ruta CPU); la unión con huecos no compensa y `maxGap` queda en 0. | fechado |
| [Paridad M3a](benchmarks/M3A_PARITY_2026-10-29.md) | Motor `core` frente al CLI en Chrome, Firefox y Node; hallazgo de `hypot`. | fechado |
| [Spike OPFS (arenas)](benchmarks/OPFS_ARENA_SPIKE_2026-10-29.md) | Modelo de pico de arenas frente al nativo y MB/s de OPFS en Chrome/Firefox (HTTP y `file://`). | fechado |
| [Modelo de arenas por nodo](benchmarks/NODE_JOB_ARENA_MODEL_2026-10-29.md) | Pico modelado de los trabajos por nodo frente al nativo (variantes a, b, c) y fórmula para M6. | fechado |
| [Cuota OPFS](benchmarks/OPFS_QUOTA_2026-10-29.md) | Cuota real de OPFS y `persist()` en Chrome y Firefox, y si cabe Majadas. | fechado |
| [Spike OPFS (un fichero por spool)](benchmarks/OPFS_SPOOL_FILES_2026-10-29.md) | Ficheros vivos, handles síncronos concurrentes y coste por fichero en Chrome/Firefox. | fechado |

## Render: línea base, R1 y R2

| Documento | Contenido | Marca |
| --- | --- | --- |
| [Línea base de render](benchmarks/RENDER_BASELINE_2026-10-30.md) | Arnés de órbita en Chrome y primer A/B de las mejoras R1 en Majadas y Belvedere. | fechado |
| [Etapa retenida al mover (R1-T1a)](benchmarks/RENDER_R1_T1A_01-11-2026.md) | Regresiones de etapa/prioridad y humo Majadas con worker y respaldo. | fechado |
| [Prioridad sobre pendientes (R1-T3b)](benchmarks/RENDER_R1_T3B_01-11-2026.md) | Coste de proyección, equivalencia de orden y humo Majadas. | fechado |
| [Medida después de R1 (R1-T7)](benchmarks/RENDER_R1_T7_01-11-2026.md) | Matriz R0 tras R1, comparación con R2-T5 y contraste de expulsiones con revisiones anteriores. | fechado |
| [Edad de escena tras R1 (R1-T7)](benchmarks/RENDER_R1_T7_SCENE_AGE_01-11-2026.md) | Por qué sube `sceneAgeMs` desde T1a y por qué se resuelve en R3. | fechado |
| [Worker de construcción (R2)](benchmarks/RENDER_BUILD_WORKER_2026-10-30.md) | Escenas construidas fuera del hilo de render, activado y no: respuestas de frame y tiempo a objetivo en Majadas y Belvedere. | fechado |
| [Digest del worker de construcción](benchmarks/RENDER_BUILD_WORKER_DIGEST_2026-10-30.md) | Digest en reposo distinto entre ambas rutas: la causa es la cámara de la sonda, no el worker. | fechado |
| [Diagnóstico de T4](benchmarks/RENDER_T4_DIAGNOSIS_2026-10-30.md) | Causa de la regresión de T4 en Belvedere: orden de carga de los bloques pendientes. | fechado |

## R3: render y LOD por celda

| Documento | Contenido | Marca |
| --- | --- | --- |
| [R3-T0: línea base CPU independiente](benchmarks/R3_T0_CPU_04-11-2026.md) | Nueve muestras en el entorno actual; no se mezclan con la captura parcial. | completado |
| [R3-T0: fixtures en el anfitrión web real](benchmarks/R3_T0_WEB_REAL_04-11-2026.md) | Seis aperturas; jerarquía diferida e injerto en v7.5, índice completo en v7.4. | completado |
| [R3-T0: fixtures y línea base CPU parcial](benchmarks/R3_T0_PARTIAL_04-11-2026.md) | Fixtures en ambas versiones y tres muestras CPU en el entorno anterior. | parcial |
| [R3-T1: alternativas de profundidad](benchmarks/R3_T1_OPTIONS_04-11-2026.md) | Diagnóstico de 16 hojas filtradas y propuesta sujeta a aprobación. | propuesta |
| [R3-T1: repetición del benchmark en Majadas](benchmarks/R3_T1_BENCH_04-11-2026.md) | Salidas completas y confirmación del criterio de parada. | parada |
| [R3-T1: parada por profundidad en Majadas](benchmarks/R3_T1_STOP_04-11-2026.md) | Colisiones entre posiciones `f32` distintas y duplicados en el límite de profundidad. | parada |
| [R3: experimento CPU de celdas adaptativas](benchmarks/R3_ADAPTIVE_EXPERIMENT_04-11-2026.md) | V5 entre ramas y celdas v7.5 con continuación adaptativa; Belvedere falla por 22 bits. | fechado |
| [R3: rango de hojas](benchmarks/R3_LEAF_SPAN_04-11-2026.md) | Tres hojas de Belvedere necesitan 22 bits relativos; ninguna en Majadas. | fechado |
| [R3: subregiones experimentales](benchmarks/R3_SUBREGIONS_04-11-2026.md) | Conteos y tiempos nativos de una ejecución con clave por subregiones. | fechado |
| [R3: scratch compacto de una región](benchmarks/R3_LAYOUT_COMPACT_05-11-2026.md) | Registro de 48 B cuando el nodo cabe en una región; comparación nativa emparejada. | fechado |
| [R3-T5: spike de GPU en la Radeon 880M](benchmarks/R3_GPU_SPIKE_05-11-2026.md) | Primer tiempo de GPU: pool real con 12 B, 16 B y 40 B, *overdraw* y celdas visibles; salta el criterio de parada. | parada |
| [R3-T7d: coste de CPU del pool y tamaños exactos](benchmarks/R3_T7D_CPU_06-11-2026.md) | Plan de residencia ≤ 1,38 ms por frame a 4 px (antes 3,28) y sin recorte falso; Majadas a 1 px en movimiento, 2,76 ms. | completado |
| [R3-T9: `LodDataset` con celdas en nativo](benchmarks/R3_T9_CELLS_06-11-2026.md) | 9 casos asentados sin huecos ni retención; celdas ≤ 303 MiB de CPU en Majadas a 4 px; construcción de un nodo hasta 69 ms en p95. | completado |
| [R3-T10: celdas entre workers en el navegador](benchmarks/R3_T10_WORKERS_07-11-2026.md) | Mismo `cutDigestAtRest` con worker de construcción y sin él; frame p95 18,1 ms frente a 17,3 de la ruta CPU; sin worker, 100 ms (R3-Q24). | completado |
| [R3-T11: humo en navegador de la ruta de celdas](benchmarks/R3_T11_SMOKE_13-11-2026.md) | Alcanza el objetivo a 4 px en Belvedere y Majadas; Majadas a 1 px dentro de una hoja entra en bucle de olvido con el presupuesto de CPU de wasm lleno (R3-Q27). | fechado |
| [R3-B1c: bucle de olvido a 1 px](benchmarks/R3_B1C_CPU_FIT_14-11-2026.md) | Residencia por celdas, hijos en curso protegidos y bloqueo sin oscilación: Majadas a 1 px se asienta; Belvedere a 1 px tras un salto desborda el pool de GPU. | fechado |
| [R3-B1d: pool de 512 MiB](benchmarks/R3_B1D_GPU_POOL_14-11-2026.md) | Pool de 65.535 páginas con presupuesto propio y corte ajustado al pool: se asientan las 14 poses a 1 y 4 px; GTT pico ~+750 MiB. | fechado |
| [R3-B1b: Hi-Z y tiempo de GPU](benchmarks/R3_B1B_HIZ_14-11-2026.md) | Fallo de escala de `hzb.rs` corregido, margen del Hi-Z derivado (+3 % de GPU) y tiempo de GPU en wasm: p95 7,38 ms en Majadas dentro de una hoja a 4 px. | fechado |
| [R3-B2: modos de color en celdas](benchmarks/R3_B2_COLOR_14-11-2026.md) | Todos los modos con celdas y función de color común (paridad con diferencia 0); modos de shader sin reconstruir; atributos con generación de color. | fechado |
| [R3-B2b: intensidad y clase en la ruta asíncrona](benchmarks/R3_B2B_ASYNC_ATTRS_14-11-2026.md) | Corrige B2: el worker de construcción no leía intensidad ni clase en modo RGB; ahora intensidad y clasificación coinciden con la ruta CPU en el navegador. | fechado |
| [R3-B3: continuidad por nube](benchmarks/R3_B3_CONTINUITY_14-11-2026.md) | Mínimo de cubo de 1 × el espaciado de referencia, sin reconstruir al alternar; efecto visual mínimo en Belvedere y Majadas. | fechado |
| [R3-B4: modos de px](benchmarks/R3_B4_PX_MODES_14-11-2026.md) | Fijo progresivo por defecto y dinámico 1–8 px hacia 16,7 ms con el tiempo de GPU; en Chrome refina y engrosa según la carga. | fechado |
| [R3-B5: pérdida de dispositivo](benchmarks/R3_B5_DEVICE_LOST_14-11-2026.md) | Recuperación sin recargar en 0,61 s tras una caída del proceso GPU de Chrome, con la misma pose y el mismo corte; canvas sustituido por la API. | fechado |
| [R3-T17: medida de la ruta de celdas](benchmarks/R3_T17_MEASURE_14-11-2026.md) | Celdas frente a CPU, local y RED-REF: RQ-01 y RQ-05 pasan; RQ-02 salvo +2,7 pp de píxeles vacíos en Majadas. | fechado |
| [R3-T7c: medida nativa del pool de celdas](benchmarks/R3_T7C_NATIVE_06-11-2026.md) | `kpc` de producción en 3,28 ms en el peor caso (spike: 4,39 ms); el plan de residencia cuesta hasta 8,75 ms de CPU y la estimación de tamaños recorta de más. | parada |
| [R3-T6: aceptación de la instancia de 16 B](benchmarks/R3_T6_ACCEPT_05-11-2026.md) | Páginas idénticas al spike en 22 casos, decodificación bit a bit en la Radeon 880M (Vulkan y GL) y en llvmpipe con `fma()` explícito y origen relativo al ojo (≤ 0,0006 px). | completado |
| [R3-T16a: compactación y Hi-Z sobre el pool](benchmarks/R3_T16_OCCLUSION_05-11-2026.md) | Compactación en GPU con Hi-Z por celda en dos fases: Majadas dentro de una hoja a 4 px de 21,66 a 4,39 ms, sin agujeros; el Hi-Z por página no aporta. | completado |
| [R3-T15: aceptación del nivel por octante](benchmarks/R3_T15_ACCEPT_05-11-2026.md) | 0 diferencias por octante entre `cell_cut` y el oráculo del spike; rangos de dibujo por corte. | completado |
| [R3-T15: bytes residentes con nivel por octante](benchmarks/R3_T15_RESIDENT_05-11-2026.md) | Niveles completos residentes; `octant_tight` cabe en 256 MiB (238,9 MiB en el peor caso). | fechado |
| [R3: opciones para no dibujar celdas ocultas](benchmarks/R3_OCCLUSION_OPTIONS_05-11-2026.md) | HZB en dos pasadas, raymarching, híbrido o puntos por cómputo; estado actual y qué medir (OQ-08). | evaluación |
| [R3-T4: celdas por encima del objetivo](benchmarks/R3_T4_OFFAXIS_05-11-2026.md) | Protección dentro del octante y factor fuera de eje; el factor global hace fallar Majadas, las demás variantes pasan. | fechado |
| [R3 fase 0a: LOD por celda en CPU](benchmarks/R3_CELL_LOD_0A_15-11-2026.md) | `cell_lod` frente a `octant_tight` y la CPU en Belvedere y Majadas, con zoom; el rango contiguo da 531 MiB en Majadas a 1 px (> 512 MiB); con la residencia R2, 294 MiB; variantes de 3 niveles (16-11-2026). | parada; resuelta con R2 |
| [R3 fase 2a: métrica de refinado por celda](benchmarks/R3_FASE2A_METRICA_16-11-2026.md) | `bound`, `edge` y `distance` frente a los criterios de parada de la fase 2a en Belvedere y Majadas; pasa `edge` (≤ 0,006 % sobre el objetivo dentro), `bound` falla con 0,596 % en Majadas a 8 px (16-11-2026). | fechado |
| [R3 fase 2b: borde recto de la pose P4](benchmarks/R3_FASE2B_BORDE_P4_18-11-2026.md) | Pares de celdas a los dos lados de los bordes de nodo visibles en P4: 0 con más de un nivel de diferencia (3028 con 0, 330 con 1), con las dos reglas de activos; el borde se ve porque el rango del nodo que contiene el ojo empieza un nivel más fino de lo que pide su parte visible: lo fija una esquina fuera de pantalla (18-11-2026). | fechado |
| [R3: bytes residentes del modo celda en Majadas](benchmarks/R3_MAJADAS_RESIDENCIA_18-11-2026.md) | Pool del modo celda en Majadas a 1–8 px, dentro de una hoja y rasante, con las dos reglas de activos: 361 MiB de pico de 512 en el peor caso, sin recortes (OQ-14, 18-11-2026). | fechado |
| [R3: línea base del modo celda antes de retirar el modo octante](benchmarks/R3_LINEA_BASE_CELDA_18-11-2026.md) | Tests `real_sim` de Belvedere (tres en modo celda, cuatro simulaciones, con sus contadores de borde y resúmenes), recuento de tests (410 y 185 pasan) y lista de nombres, para comprobar que la retirada del modo octante no cambia el modo celda (18-11-2026). | fechado |
| [R3: niveles que pide la regla de la GPU frente al rango por nodo](benchmarks/R3_RANGO_GEMELO_18-11-2026.md) | Poses P1 a P5 de la muestra: el rango de hoy no contiene lo pedido junto al ojo (`lo` hasta 3 niveles fino, `hi` hasta 6 corto); una cota de fórmula cerrada lo contiene en 46 de 46 casos; el `hi` junto al ojo es cuestión de presupuesto (18-11-2026). | fechado |
| [R3: el extremo fino del rango limitado por datos](benchmarks/R3_EXTREMO_FINO_POR_DATOS_18-11-2026.md) | `hi` calculado en la celda ocupada más cercana: cota válida en 44 de 44 casos con holgura de 0 a 1 nivel, 4 o 5 niveles bajo la cota fiel junto al ojo; el coste real en el pool queda sin medir porque el estimador no describe la residencia (OQ-16, 18-11-2026). | fechado |
| [R3: fuentes baratas para limitar el extremo fino por datos](benchmarks/R3_FUENTES_COTA_POR_DATOS_18-11-2026.md) | Cuatro fuentes de la celda ocupada más cercana, con y sin recorte a la vista: solo sirven las cajas de página del nivel residente más fino recortadas (exactas en el nodo del ojo, 3 o 4 niveles holgadas en el contiguo); sin recorte todas vuelven al último nivel (18-11-2026). | fechado |
| [R3: el tope por datos conectado al deseo de las celdas](benchmarks/R3_TOPE_POR_DATOS_19-11-2026.md) | La cota por datos limita ya lo que se pide: imagen en reposo igual (≤ 0,119 % en P1 a P4), ninguna muestra dibujable más gruesa, 288 páginas menos en P2 y 72 menos en Majadas a 1 px; la cota se redefine para no depender del orden de llegada de los niveles (19-11-2026). | fechado |
| [R3: cierre del plan del rango por nodo](benchmarks/R3_CIERRE_RANGO_POR_NODO_19-11-2026.md) | `lo` y cota por datos solo con octantes a la vista (#87) y retirada de `Window`; el tramo único como fuente de la emisión se aparcó (28 tests en rojo, reglas que faltaban; PR #88) y el arreglo de «un solo tramo» se revirtió porque hacía dibujar demasiado fino (19-11-2026). | fechado |
| [R3, fase 2c: transiciones de nivel en modo celda](benchmarks/R3_FASE2C_TRANSICIONES_19-11-2026.md) | Puertas del ADR 0053 en el gemelo: espacial, 0 pares visibles con más de un nivel en Belvedere y 48–855 por simulación en el octree sintético; temporal, 280 saltos dentro del nodo en una órbita real y 62–1338 en el sintético (OQ-17). | fechado |
| [R3 fase 2a: selección por celda en la GPU](benchmarks/R3_FASE2A_LOD_POR_CELDA_16-11-2026.md) | Puerta de partición (0 huecos/0 dobles), defectos del modo `cell` corregidos, contadores por profundidad en bench nativo y probe de navegador `octant`/`cell` en Belvedere y comparación en navegador: bandas sin corte en `cell` (16-11-2026). | cerrado |
| [R3 fase 1: niveles a demanda](benchmarks/R3_FASE1_NIVELES_A_DEMANDA_16-11-2026.md) | Cierre de la fase 1 (#35): escenario de pares cercanos, puerta de engrosamientos, navegador con `fs_e430` y los fallos anteriores arreglados (dos de R3-Q21). | vigente |
| [R3-T4: alternativas al error proyectado](benchmarks/R3_T4_METRIC_05-11-2026.md) | Métrica ajustada y nivel por octante frente a la cota; `octant_tight` casi iguala a la CPU salvo 2,27× a 1 px. | fechado |
| [R3-T4: spike de cobertura nativo](benchmarks/R3_T4_COVERAGE_05-11-2026.md) | Instancias, bytes y `DescendantTooFine` por pose en Belvedere y Majadas; saltan los criterios de parada. | parada |
| [R3-T1: aceptación de celdas por nodo](benchmarks/R3_T1_ACCEPT_05-11-2026.md) | Ninguna hoja sin separar en Belvedere y Majadas completos con la continuación adaptativa. | fechado |
| [R3: layout de páginas por anchura de región](benchmarks/R3_REGION_LAYOUT_05-11-2026.md) | Anchuras 8–32 con flags por página o por instancia sobre Belvedere y Majadas completos; gana W = 10 con instancia de 12 B. | fechado |
| [Celdas por nodo sobre datos reales (R3-T1)](benchmarks/RENDER_R3_T1_NODE_CELLS_01-11-2026.md) | Longitud y bytes de la cadena de hoja en Majadas y Belvedere; criterios de parada de R3. | fechado |
| [Hojas en el tope de R3-T1 (exploración)](benchmarks/EXPLORE_ADAPTIVE_LEAF_02-11-2026.md) | Meseta de espaciado con 8 celdas y bench según ADR 0028: 0,06 % y 0,14 % en el tope. | fechado |

## R4 y R5: pirámide densa y jerarquía paginada

| Documento | Contenido | Marca |
| --- | --- | --- |
| [Spike de densidad de la pirámide (R4-T1)](benchmarks/R4_DENSITY_SPIKE_02-11-2026.md) | Hoja × `sample_grid` en Majadas y Belvedere: salto de espaciado, disco, RSS y tiempo. | fechado |
| [Memoria del conversor con g = 256 (R4-T1)](benchmarks/R4_SPARSE_GRID_RSS_02-11-2026.md) | Causa y corrección de la RSS (umbral `mmap` de glibc, rejilla dispersa compacta). | fechado |
| [Tiempo del conversor con g = 256 (R4-T1)](benchmarks/R4_CONVERT_TIME_02-11-2026.md) | Escaneo raíz en paralelo y ventana deslizante de bloques. | fechado |
| [Tiempo del conversor, segunda ronda (R4-T1)](benchmarks/R4_CONVERT_TIME_2_02-11-2026.md) | Celda de muestreo en una pasada y tabla dispersa compacta; Majadas +24 % frente a V5. | fechado |
| [Conversión v7.4-pre frente a V5 (R4-T4b)](benchmarks/R4_T4B_CONVERSION_02-11-2026.md) | Tiempo, RSS y disco de Majadas y Belvedere; tiempo +92–115 %. | fechado |
| [Matriz de render y R3-T1 sobre v7.4-pre (R4-T6)](benchmarks/R4_T6_MEASUREMENT_02-11-2026.md) | Regresión de píxeles vacíos en Belvedere; contadores anómalos; hojas en el tope 1,4–2,0 %. | fechado |
| [Matriz de render tras los arreglos del visor (R4-T6b)](benchmarks/R4_T6B_MEASUREMENT_02-11-2026.md) | Pasa los criterios de render; píxeles vacíos de Belvedere 0,71–0,76 → 0,29–0,36; R3 sigue en pausa. | fechado |
| [Tamaño de página de la jerarquía paginada (R5-P0)](benchmarks/R5_P0_PAGES_02-11-2026.md) | k = 5 vale en aéreo (p95 16–27 KB), no con ramificación 8; índice −50 %. | fechado |
| [Bytes de la cadena de hoja (exploración)](benchmarks/EXPLORE_LEAF_CHAIN_BYTES_02-11-2026.md) | Poda de niveles casi duplicados: 0 hojas > 4 ×, cadena ≈ 1 × puntos. | fechado |
| [Apertura remota por HTTP Range (R5-P4)](benchmarks/R5_P4_HTTP_RANGE_02-11-2026.md) | RED-REF: primer frame 370–390 ms y 4 px ≤ 1,3 s con v7.5-pre; RQ-04 pasa; v7.4 como base. | fechado |

## Informes y benchmarks originales

| Documento | Contenido | Marca |
| --- | --- | --- |
| [Web](benchmarks/WEB_PERFORMANCE_BENCH.md) | Métricas de navegador, flujos y limitaciones visuales. | histórico |
| [Memoria v5](benchmarks/V5_MEMORY_BENCH.md) | Conversión, acceso a bloques, memoria y perfiles nativos. | histórico |
| [Vóxeles](benchmarks/VOXEL_BUILD_BENCH.md) | Construcción, memoria conjunta y reutilización de celdas. | histórico |
| [Almacenamiento](benchmarks/STORAGE_BENCH.md) | Suite repetida: tamaños, escritura, reconstrucción, carga de hijos. | histórico |
| [Preparación v6](benchmarks/STORAGE_V6_PREPARATION.md) | Traits, compatibilidad y criterios de v6. | histórico |
| [Pre3](benchmarks/STORAGE_V6_PRE3_BENCH.md), [pre4](benchmarks/STORAGE_V6_PRE4_BENCH.md), [SensOFOREST](benchmarks/SENSOFOREST_V6_BENCH.md) | Parejas válidas, tiempos, lectura selectiva y RSS. | histórico |
| [I/O v6](benchmarks/STORAGE_V6_IO.md), [CPU](benchmarks/STORAGE_V6_CPU.md), [planos de bytes](benchmarks/STORAGE_V6_BYTE_PLANES.md) | Pipeline optativo, CRC/diccionarios y localidad (tiling descartado). | histórico |
| [Diccionarios](benchmarks/ATTRIBUTE_DICTIONARY_BENCH.md) | Experimento de diccionarios de atributos. | histórico |
| [Características nightly](benchmarks/nightly-features-2026-10-28.md) | Evaluación de `portable_simd` en CRC32 y planos de bytes (ninguna adoptada). | fechado |
| [Ocupación cuantizada](benchmarks/QUANTIZED_OCCUPANCY_BENCH.md) | Residuales exactos y coste del índice. | histórico |
| [Selección LOD](benchmarks/LOD_FRONTIER_BENCH.md) | Frontera y prioridad incremental. | histórico |
| [Datos reales](benchmarks/REAL_DATASET_BENCH.md) | Protocolo y resultados con un escaneo grande. | histórico |
| [Ramificación espacial](benchmarks/SPATIAL_BRANCHING_BENCH.md), [anchura `u16`](benchmarks/COORDINATE_WIDTH_BENCH.md) | Experimentos descartados o acotados. | histórico |

## Archivo `archive/`

| Documento | Contenido | Marca |
| --- | --- | --- |
| [V6 pre1](archive/FORMAT_V6_PRE1.md), [pre2](archive/FORMAT_V6_PRE2.md), [pre3](archive/FORMAT_V6_PRE3.md), [pre4](archive/FORMAT_V6_PRE4.md), [V4](archive/FORMAT_V4.md) | Especificaciones de formatos retirados o en retirada. | histórico |
| [Entregas 14-10-2026](archive/DELIVERIES_2026-10-14.md) | Implementaciones y experimentos cerrados, con checks históricos. | histórico |
| [Evaluación 24-10-2026](archive/CODE_ASSESSMENT_2026-10-24.md) | Contraste de documentación, checks y próximos pasos. | histórico |
| [Evaluación de render 30-10-2026](archive/RENDER_ASSESSMENT_2026-10-30.md) | Carga y render a escala (cientos de GB, 1–8 px): mapa, hallazgos B1–B7, investigación externa y mejoras Q1–Q8. | histórico |
| [Evaluación 28-10-2026](archive/CODE_ASSESSMENT_2026-10-28.md) | Revisión por áreas, plan de 15 tareas, decisiones, verificación y pendientes. | histórico |
| [Historial técnico](archive/DEVELOPMENT_HISTORY.md) | Snapshot anterior a v5. | histórico |
| [Plan V7](archive/format-v7-plan.md) | Contrato, recursos acotados, async/paralelismo, candidatos de formato y retirada de V5/V6. | histórico |
| [Render y carga a escala](archive/render-y-carga-escala.md) | Fases R0–R5: arnés y línea base, mejoras de LOD y carga (R1), worker de construcción (R2) y fases decididas R3–R5 (ADR 0019–0023). | histórico |
| [D3c: congelado de V7 final](archive/d3c-congelado-v7-final.md) | Identidad final V7 sobre v7.5-pre con el escritor híbrido, salida predeterminada y regeneración de datasets y referencias (ADR 0044). | histórico |
| [Publicación del repositorio](archive/publicacion-del-repositorio.md) | Repositorio público: portada, documentación, demo experimental en GitHub Pages y ajustes de GitHub (ADR 0045). | histórico |
| [Muestra de la demo en V7](archive/muestra-de-la-demo-en-v7.md) | La muestra integrada pasa a V7 y a 589.824 puntos; V5 queda como heredado hasta V8 (ADR 0047). | histórico |
| [Fase 2a: LOD por celda en la GPU](archive/fase-2a-lod-por-celda-en-gpu.md) | Plan detallado de la fase 2a (#44): gemelo Rust de la decisión por celda, medida de la métrica, corte en modo celda, decisión en el paso de ranuras y opción `cellLod`. | histórico |
| [Fase 2b: rango de niveles y transiciones](archive/fase-2b-rango-de-niveles-y-transiciones.md) | Plan de la fase 2b (#44), sustituido por el replanteamiento y el ADR 0055. | histórico |
| [Replanteamiento del LOD por celda](archive/replanteamiento-lod-por-celda-18-11-2026.md) | Por qué se atascó la fase 2b, requisitos replanteados según su intención y experimentos E-A/E-B1 (ADR 0055). | histórico |
| [Limpieza y refactor](archive/limpieza-y-refactor-18-11-2026.md) | Limpieza de disco y git, puesta al día de documentos vivos y división mecánica de los módulos más grandes, sin cambios de comportamiento; decisiones pendientes del propietario. | histórico |
| [Retirada del modo octante](archive/retirada-del-modo-octante.md) | Ejecuta el ADR 0056: línea base del modo celda, cambio de predeterminado, ensayo que clasifica los tests, oráculo de pasos de celda, borrado de la API y de las ramas `LodMode`; diez tareas, pre-mortem aplicado. | histórico |
| [ADR 0055 en modo celda: rango por nodo y puerta de residencia](archive/adr-0055-rango-por-nodo.md) | División de `cell_pool.rs`, rango por nodo en lugar de sondas por octante y puerta de residencia en los bordes; doce tareas en tres tramos. Hechas las tareas 1 a 9, la retirada de `Window` y #87; el tramo único como fuente de la emisión y el borrado de la maquinaria por octante se aparcan (#88). | histórico |
| [Temporales del conversor, fase B](archive/temporales-del-conversor-fase-b.md) | Plan ejecutado de los spools comprimidos (planos de bytes + LZ4) y de la medida de temporales; fusionado en #108 ([registro](benchmarks/TEMPORALES_CLI_FASE_B_20-11-2026.md)). | histórico |
| [Demo: color estable, WASD, maquetación y segunda nube](archive/demo-color-wasd-maquetacion.md) | Rangos de color independientes de la vista, vuelo WASD respecto a la cámara, visor a pantalla completa, recorte real de Belvedere y lints pedantes (#89); fusionado en #91 y #92. | histórico |
| [D4: fuente `{url}` en la API pública](archive/d4-fuente-url-en-la-api.md) | Contrato C1–C6 y resultado: `load` de la API abre datasets remotos por HTTP Range (TW8, #11); `sidecarUrl`. | histórico |
| [Cómo retomar (18-11-2026)](archive/retomar-18-11-2026.md) | Ramas locales y su estado, cómo comprobarlas, las respuestas del propietario a Q1–Q5 y D1–D6 y el siguiente trabajo en orden. | histórico |
| [R3: cubos por nodo en GPU](archive/render-r3-cubos-en-gpu.md) | Celdas por nodo independientes de la cámara en un pool de GPU paginado (ADR 0027), spikes primero, modos de huecos y de px. | histórico |
| [R3: anchura de región, layout, máscaras y flags](archive/r3-anchura-de-region.md) | Experimento: anchura de página 8–32, ubicación de flags y modelos de página; decidido en ADR 0032. | histórico |
| [R4: pirámide densa y rejilla global](archive/render-r4-piramide-densa.md) | Revisión `v7.4-pre`: hojas pequeñas, espaciado por nodo, rejilla anclada en el CRS y CRS completo; spike de densidad primero. | histórico |
| [R5: jerarquía paginada](archive/render-r5-jerarquia-paginada.md) | Revisión `v7.5-pre`: páginas por niveles desde anclas, k por página, lector perezoso local y HTTP Range; flags LAS y Extra Bytes. | histórico |
| [Etapa 1](archive/conversor-web-paralelo-etapa1.md), [2](archive/conversor-web-paralelo-etapa2.md), [3](archive/conversor-web-paralelo-etapa3.md), [4](archive/conversor-web-paralelo-etapa4.md) del conversor web | S0/M0/S1; trabajos por nodo, S8, S9 y M1; S2, S10, M2 y M3a; M3b con zonas OPFS y retirada del conversor antiguo. | histórico |
| [Dependencias del renderer](archive/RENDERER_DEPENDENCIES.md) | Migración a wgpu 30/glam 0.34 (14-10-2026), checks y validación parcial de navegador; decisión en el ADR 0010. | histórico |
| [Formato V8](archive/formato-v8.md) | Diseño de la V8 (aprobado): color de pantalla sin duplicar, LOD solo con atributos de dibujo, distancia mínima entre puntos en la cabecera, solo LZ4 tras medir zstd ([ADR 0060](../adr/0060-v8-solo-lz4.md)) y retirada de V5, v6 y v7. | histórico |
| [Formato V8, v8.1-pre](archive/formato-v8-pre1.md) | Plan de implementación de la primera revisión experimental: identidad, cuantiles de distancia, LOD con columnas de dibujo, color de pantalla derivado y lectura en el visor; ejecutado y medido. | histórico |
| [Formato V8, v8.2-pre](archive/formato-v8-pre2.md) | Experimento de D3: zstd en columnas de `.octattrs` con cuatro esquemas fijos (`lz4`, `cold`, `cold-rgb`, `all`) medidos en tamaño, conversión, carga y render; resultado: solo LZ4 (ADR 0060). | histórico |
| [Formato V8 congelado (D4)](archive/formato-v8-congelado.md) | Identidad final V8 sobre v8.1-pre y retirada de v5, v6, v7, las revisiones previas, zstd, EPT y checkpoints. | histórico |
