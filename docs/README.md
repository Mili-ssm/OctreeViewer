# Índice de documentación

Punto de entrada del proyecto: [README](../README.md) (estado y prioridades activas). Marcas:
**vivo** = se mantiene con el código; **histórico** = registro fechado, no se edita ([política](../CONTRIBUTING.md#convenciones)).

## Referencia

| Documento | Contenido | Marca |
| --- | --- | --- |
| [Inventario](reference/inventory.md) | Mapa de componentes, identidades de formato, herramientas, límites y candidatos. | vivo |
| [Formato V5](reference/format/FORMAT_V5.md) | Especificación del `.oct`/`.octattrs` predeterminado. | vivo |
| [V7.1-pre](reference/format/FORMAT_V7_PRE1.md), [V7.2-pre](reference/format/FORMAT_V7_PRE2.md), [V7.3-pre](reference/format/FORMAT_V7_PRE3.md) | Especificaciones de las revisiones V7 experimentales. | vivo |

## Explicación

| Documento | Contenido | Marca |
| --- | --- | --- |
| [Plan V7](explanation/format-v7-plan.md) | Recursos acotados, async/paralelismo, candidatos de formato y retirada de V5/V6. | vivo |
| [Resolución regional LOD](explanation/REGIONAL_VOXEL_LOD.md) | Contrato de celdas, fallback y regresiones del LOD Replacement. | vivo |
| [Resúmenes por nodo](explanation/NODE_ATTRIBUTE_SUMMARIES.md) | Contenedores experimentales de resúmenes y diccionario conjunto. | vivo |
| [Dependencias del renderer](explanation/RENDERER_DEPENDENCIES.md) | Migración wgpu 30/glam 0.34, checks y validación parcial de navegador. | vivo |

## Registros

Benchmarks en [records/benchmarks](records/benchmarks/); sus cifras pertenecen a la revisión, perfil y equipo con que se midieron.

| Documento | Contenido | Marca |
| --- | --- | --- |
| [Evidencia V7](records/benchmarks/format-v7-evidence.md) | Evidencia por incremento del plan V7; datos en [data/](records/benchmarks/data/). | vivo (se amplía) |
| [V7.3-pre frente a V5](records/benchmarks/STORAGE_V7_V73_VS_V5_2026-10-28.md) | Conversión y lectura completas en Belvedere y Majadas, 3 rondas; disco menor, tiempo/RSS mayores. | fechado |
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
| [Evaluación 28-10-2026](records/archive/CODE_ASSESSMENT_2026-10-28.md) | Revisión por áreas, plan de 15 tareas, decisiones, verificación y pendientes. | histórico |
| [Historial técnico](records/archive/DEVELOPMENT_HISTORY.md) | Snapshot anterior a v5. | histórico |

## Decisiones

[ADR](adr/README.md): decisiones de formato, versiones, límites, toolchain y seguridad, con alternativas y condiciones de revisión (vivo).

## Datos y recursos

[Datasets de escala](../data/datasets/README.md) y [muestras](../data/samples/README.md) (procedencia y preparación); [host web](../examples/web-host/README.md) (API, build, probes; en inglés); [capturas](point-cloud-previews/) de nubes de ejemplo.
