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
| [V7.1-pre](reference/format/FORMAT_V7_PRE1.md), [V7.2-pre](reference/format/FORMAT_V7_PRE2.md), [V7.3-pre](reference/format/FORMAT_V7_PRE3.md) | Especificaciones de las revisiones V7 experimentales. | vivo |

## Planes y specs

Cada plan lleva en la línea 3 `Estado: activo | pendiente | en pausa | cerrado · dd-mm-aaaa`
(`grep -n '^Estado:' docs/plans/*.md` da el estado de todos). Los cerrados no se reescriben: el
trabajo nuevo va en un plan nuevo.

| Documento | Contenido | Estado |
| --- | --- | --- |
| [Plan V7](plans/format-v7-plan.md) | Contrato, recursos acotados, async/paralelismo, candidatos de formato y retirada de V5/V6. | activo |
| [Render y carga a escala](plans/render-y-carga-escala.md) | Fases R0–R5: arnés y línea base, mejoras de LOD y carga (R1), worker de construcción (R2) y fases decididas R3–R5 (ADR 0019–0023). | activo |
| [R3: cubos por nodo en GPU](plans/render-r3-cubos-en-gpu.md) | Celdas por nodo independientes de la cámara en un pool de GPU paginado (ADR 0027), spikes primero, modos de huecos y de px. | en pausa (ADR 0028) |
| [R4: pirámide densa y rejilla global](plans/render-r4-piramide-densa.md) | Revisión `v7.4-pre`: hojas pequeñas, espaciado por nodo, rejilla anclada en el CRS y CRS completo; spike de densidad primero. | activo |
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
| [Celdas por nodo sobre datos reales (R3-T1)](records/benchmarks/RENDER_R3_T1_NODE_CELLS_01-11-2026.md) | Longitud y bytes de la cadena de hoja en Majadas y Belvedere; criterios de parada de R3. | fechado |
| [Spike de densidad de la pirámide (R4-T1)](records/benchmarks/R4_DENSITY_SPIKE_02-11-2026.md) | Hoja × `sample_grid` en Majadas y Belvedere: salto de espaciado, disco, RSS y tiempo. | fechado |
| [Memoria del conversor con g = 256 (R4-T1)](records/benchmarks/R4_SPARSE_GRID_RSS_02-11-2026.md) | Causa y corrección de la RSS (umbral `mmap` de glibc, rejilla dispersa compacta). | fechado |
| [Tiempo del conversor con g = 256 (R4-T1)](records/benchmarks/R4_CONVERT_TIME_02-11-2026.md) | Escaneo raíz en paralelo y ventana deslizante de bloques. | fechado |
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
