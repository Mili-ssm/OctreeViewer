# Octree Viewer

Visor de nubes LiDAR en Rust, WebAssembly y WebGPU. Convierte LAS/LAZ o EPT local a una
jerarquía espacial persistente y carga los bloques necesarios para la vista. El objetivo a largo
plazo es explorar fuentes mayores de **500 GB decimales** con memoria acotada. La escala completa
y cualquier superioridad frente a Potree siguen sin demostrarse.

## Estado actual — 24-10-2026

- El formato de trabajo vigente es **v5 definitivo** (`OCTVFRM1`, revisión 0), separado en `.oct` (coordenadas y jerarquía) y
  `.octattrs` (RGB y atributos numéricos). Se lee y escribe v5; hay que regenerar archivos v1–v4 y v5 provisionales (`OCTVWFRM`) desde sus fuentes.
- Están implementados **`v6.1-pre`, `v6.2-pre`, `v6.3-pre` y `v6.4-pre` optativos**. Pre1 añade deltas; pre2 serializa
  ocupación en arrays por profundidad dentro de cada bloque, con registros de 6 bytes y residuales
  exactos. Se elige el menor payload tras LZ4, con fallback raw/delta. V5 sigue predeterminado;
  pre3 añade constantes/diccionarios exactos por columna; pre4 añade planos de bytes independientes
  y topología de máscaras con hojas/prefijos implícitos, con fallback y lectura selectiva.
  V6 final y pre5 permanecen rechazados.
- La conversión admite LAS/LAZ y EPT local. La demo web admite carga progresiva por bloques,
  color por RGB o componente numérico, cámara orbital y modo pájaro.
- La ruta LOD voxelizada usa celdas diádicas enteras sin solapamiento entre generaciones. El
  objetivo en píxeles guía la selección; no cambia arbitrariamente el tamaño físico de las celdas.
  Los huecos de la fuente no se rellenan. Este invariante todavía necesita cobertura en todos los
  consumidores de bloques.
- La migración a **wgpu/naga 30 y glam 0.34** ya compila en nativo/WASM y pasa
  los checks nativos y readback por software. La validación de navegador sigue parcial por
  pérdida de dispositivo también reproducida con un clear independiente.
  [Cambios de contrato, checks y límites](docs/RENDERER_DEPENDENCIES.md).
- `createViewer` libera el worker y los recursos registrados si falla la inicialización;
  EPT rechaza presupuestos estimados inviables al leer la jerarquía y antes de reservar
  bloques de planificación. Son correcciones locales; no certifican GPU ni un límite RSS.
- Las métricas de conversión, memoria, LOD y navegador son resultados de ejecuciones concretas,
  con límites de interpretación. No equivalen a límites garantizados ni a objetivos ya cumplidos.

## Prioridades pendientes

Esta es la única lista activa. El [inventario por componente](docs/PROJECT_INVENTORY.md)
detalla capacidades, límites y propuestas; las entregas cerradas y sus checks se conservan en
[el archivo del 14-10-2026](docs/DELIVERIES_2026-10-14.md) y en los informes temáticos.

| Orden | Trabajo abierto y componentes | Siguiente paso / criterio de cierre |
| --- | --- | --- |
| 1 | **Corrección visual y transición LOD** — core, render, WASM. | Retener por región la representación anterior hasta disponer de hijos, sin añadir huecos ni solapamientos. Ya hay [resolución regional para representantes Replacement](docs/REGIONAL_VOXEL_LOD.md): una rama gruesa permite detalle fuera de sus regiones ocupadas; fallback global acotado para payloads dispersos. Falta conservar representación entre generaciones por región y probar cortes mixtos Replacement/Additive con carga lenta/fallida, zoom y movimiento en un dataset real multibloque. Revisar también `sync_visible_blocks`. |
| 2 | **Validación de navegador** — WASM y host. | Repetir presentación en Chrome/AMD y Firefox, cámara/picking/atributos, escritorio/móvil y recuperación de errores. Confirmar modo pájaro con teclado físico. La limpieza de `createViewer` ante fallo síncrono ya tiene regresiones Node; falta comprobar los flujos reales de GPU y recuperación ([evaluación](docs/CODE_ASSESSMENT_2026-10-24.md)). Readback/picking no cierran la presentación; Chrome/AMD sigue sin presentar incluso un clear independiente en las rondas documentadas. |
| 3 | **Pico conjunto de memoria** — core, render, WASM. | Correlacionar caché decodificada, builder/proyección, instancias, escena anterior, capacidad WASM, RSS/PSS y recursos GPU durante carga y cámara. Incluir Additive real antes de decidir si retener un acumulador; los prefijos medidos no lo justifican por sí solos. |
| 4 | **Conversión y escala de extremo a extremo** — convert y host. | Completar matriz de primera vista/refinamiento con Belvedere, Majadas y Albuera; Belvedere a 4 px no cierra bajo el caché probado. Separar conversión, apertura, I/O, temporales y memoria. El rechazo temprano del presupuesto estimado EPT tiene regresiones; falta medir RSS y escala real ([evaluación](docs/CODE_ASSESSMENT_2026-10-24.md)). Más de 500 GB y comparación con Potree requieren hardware, calidad y tareas equivalentes. |
| 5 | **Requisitos de producto y soporte** — transversal. | Acordar plataformas y mínimos RAM/GPU, dimensiones LAS/LAZ (CRS, flags, Extra Bytes), licencia/compatibilidad y alcance de medición, clasificación, recorte y exportación. Documentar decisiones antes de prometer soporte. |
| 6 | **Almacenamiento v6: lectura selectiva y transporte** — format y convert. | Extender benchmark XYZ/RGB/columna a 1/2/4 lectores con presupuesto simultáneo, bytes/rangos, CPU/RSS y corpus real. Medir caché fría/HDD/remoto antes de integrar concurrencia o prefetch en el viewer; definir lotes, cancelación y fallos parciales. |
| 7 | **Siguientes candidatos v6** — core y format. | Perfilar selector de codecs; comparar grupos RGB exactos, bitpacking por rango y resúmenes/diccionarios con los codecs pre3/pre4 en la misma envoltura. Exigir parejas válidas, lectura selectiva, precisión, corrupción, checkpoints y WASM antes de otra revisión incompatible; mantener v6 final rechazado. |
| 8 | **Grilla física y varias nubes** — core y format. | Fijar CRS, unidades/ejes, ancla y escala diádica; decidir grilla global o transformación explícita por nube. Probar negativos, límites y nubes convertidas por separado, independientemente del orden de carga. La ocupación cuantizada por bloque de pre2/pre4 no resuelve este contrato. |

Las prioridades 1–4 mantienen el foco del visor. Los avances de almacenamiento no cierran la
validación visual ni la memoria conjunta. El detalle y los límites de las medidas están en los
[benchmarks web](docs/WEB_PERFORMANCE_BENCH.md), de
[vóxeles](docs/VOXEL_BUILD_BENCH.md) y de [almacenamiento](docs/STORAGE_BENCH.md).
Caras expuestas/agrupadas, índice exterior compacto y otros añadidos se mantienen como propuestas
condicionadas en el inventario, sin convertirlos en compromisos nuevos.

## Uso y formato

Conversión nativa:

```sh
cargo run --profile perf -p viewer_convert -- INPUT.laz OUTPUT.oct --format-version v5
# También admite un directorio EPT local laszip o su ept.json.
# --checkpoint conserva fases completas; --resume retoma la misma entrada/opciones.
```

La conversión produce una pareja v5 definitiva. Las muestras versionadas se han regenerado;
los datasets locales provisionales deben reconvertirse. Para ensayar las revisiones experimentales,
usar `--format-version v6.1-pre`, `--format-version v6.2-pre`, `--format-version v6.3-pre`
o `--format-version v6.4-pre`
en lugar de `v5`. Se rechazan las versiones sin backend antes
de escribir y los manifiestos de checkpoint anteriores a V5. `v6.N-pre` identifica revisiones
exactas de desarrollo; se implementan pre1/pre2/pre3/pre4, no `v6` final.

La pareja contiene:

| Archivo | Contenido | Uso |
| --- | --- | --- |
| `OUTPUT.oct` | XYZ cuantizado a `u32` (v5 directo; pre1 delta/fallback; pre2 también ocupación/residuales), marco local, jerarquía e índice común. | Se puede abrir sin atributos; se visualiza en gris. |
| `OUTPUT.octattrs` | RGB de presentación y columnas numéricas tipadas, alineadas por bloque/fila. | Opcional; habilita RGB, intensidad, clasificación y selección de componentes numéricos. |

En la demo, abrir `.oct` y seleccionar después su `.octattrs`, o asociar el acompañante antes de
abrir las coordenadas. Para desarrollo, servir el repositorio por HTTP:

```sh
python3 -m http.server 8000 --bind 127.0.0.1
# Abrir http://localhost:8000/examples/web-host/index.html
```

Para doble clic se usa el paquete autocontenido `artifacts/visor-web-local/index.html`. Contrato,
regeneración y pruebas de ambos modos: [README del host web](examples/web-host/README.md).
Especificación binaria: [formato v5](crates/viewer_format/FORMAT_V5.md). La publicación de los dos
archivos no es atómica; el conversor valida candidatos y publica atributos antes de coordenadas.

## Arquitectura y límites conocidos

| Área | Responsabilidad | Límite relevante |
| --- | --- | --- |
| `viewer_core` | Geometría, jerarquía, LOD, caché y contratos compartidos. | El índice residente y la memoria estimada no equivalen a RSS. |
| `viewer_format` | V5 y v6 pre1/pre2/pre3/pre4, cuantización, codecs, checksums y lectura selectiva. | XYZ lógico ocupa 12 bytes/punto; v6 puede reducir el stream antes de LZ4; los enteros de 64 bits conservan bits en disco, pero su proyección de color puede perder precisión. |
| `viewer_convert` | Conversión LAS/LAZ con spools/checkpoints y EPT local. | EPT remoto, binary/zstandard y checkpoints EPT no están soportados. |
| `viewer_render_wgpu` | Cámara, voxelización, buffers GPU, HZB y presentación. | Los presupuestos propios no representan toda la VRAM; la GPU puede reservar recursos fuera del control del renderer. |
| `viewer_wasm` | Worker, carga LOD y conversión web en dos pasadas. | Lecturas por bloque síncronas; convertir fuentes de cientos de GB en navegador no es requisito. |
| `examples/web-host` | Demo, API embebible, carga y descarga de archivos. | OPFS/cuota, pérdida de GPU, picking y cancelación requieren validación real en navegadores objetivo. |

La conversión conserva intensidad, clasificación, retornos, ángulo, user data y point source ID; si
existen, GPS time, RGB16 y NIR. Flags LAS, Extra Bytes y CRS requieren trabajo adicional y las
omisiones se notifican. El `.oct` conserva marco local `f64` y error de cuantización acotado; el
render reconstruye posiciones locales `f32`.

## Documentación

Punto de entrada para retomar trabajo: [inventario del proyecto](docs/PROJECT_INVENTORY.md).
Incluye mapa de código, herramientas, cobertura disponible y propuestas de mejora.
La [evaluación del código del 24-10-2026](docs/CODE_ASSESSMENT_2026-10-24.md) contrasta
documentación, criterios de desarrollo, checks y próximos pasos; las prioridades activas
siguen siendo las de este README.

| Documento | Contenido vigente |
| --- | --- |
| [Preparación v6](docs/STORAGE_V6_PREPARATION.md) y [pre1](crates/viewer_format/FORMAT_V6_PRE1.md) / [pre2](crates/viewer_format/FORMAT_V6_PRE2.md) / [pre3](crates/viewer_format/FORMAT_V6_PRE3.md) / [pre4](crates/viewer_format/FORMAT_V6_PRE4.md) | Traits, formatos experimentales, compatibilidad y criterios pendientes. |
| [Pre3](docs/STORAGE_V6_PRE3_BENCH.md), [pre4](docs/STORAGE_V6_PRE4_BENCH.md) y [SensOFOREST completo](docs/SENSOFOREST_V6_BENCH.md) | Comparaciones de parejas válidas, tiempos, lectura selectiva y RSS con sus límites. |
| [I/O v6](docs/STORAGE_V6_IO.md), [CPU/CRC/diccionarios](docs/STORAGE_V6_CPU.md) y [planos de bytes](docs/STORAGE_V6_BYTE_PLANES.md) | Pipeline optativo, lectores del benchmark y optimizaciones compatibles; tiling descartado para la ruta actual. |
| [Resúmenes por nodo](docs/NODE_ATTRIBUTE_SUMMARIES.md) y [diccionarios](docs/ATTRIBUTE_DICTIONARY_BENCH.md) | Experimentos; no equivalen a integración en `.oct`/`.octattrs`. |
| [Benchmarks de almacenamiento](docs/STORAGE_BENCH.md) | Suite repetida: tamaños, escritura, reconstrucción y carga secuencial de hijos. |
| [Ocupación cuantizada](docs/QUANTIZED_OCCUPANCY_BENCH.md) | Residuales exactos, coste del índice en memoria y límites del candidato. |
| [Benchmark v5 de memoria y lectura](docs/V5_MEMORY_BENCH.md) | Conversión, acceso a bloques, memoria y perfiles nativos. |
| [Benchmark de construcción de vóxeles](docs/VOXEL_BUILD_BENCH.md) | Construcción, memoria conjunta, acumuladores y experimentos de reutilización. |
| [Resolución regional LOD](docs/REGIONAL_VOXEL_LOD.md) | Contrato de celdas, fallback, regresiones, probe WebGPU y comparación nativa de cortes Belvedere. |
| [Dependencias del renderer](docs/RENDERER_DEPENDENCIES.md) | Migración wgpu 30/glam 0.34, contratos de superficie/readback y validación parcial de navegador. |
| [Benchmark web](docs/WEB_PERFORMANCE_BENCH.md) | Métricas de navegador, flujos, resultados y limitaciones visuales. |
| [Selección LOD](docs/LOD_FRONTIER_BENCH.md) | Contratos y mediciones de selección de frontera y prioridad incremental. |
| [Datos reales](docs/REAL_DATASET_BENCH.md) | Protocolo y resultados históricos; revisar fecha/versión antes de comparar con v5. |
| [Ramificación espacial](docs/SPATIAL_BRANCHING_BENCH.md) | Experimentos de subdivisión y cortes con diferentes ramas. |
| [Anchura de coordenadas](docs/COORDINATE_WIDTH_BENCH.md) | Evaluación del experimento `u16` y sus límites. |
| [Entregas del 14-10-2026](docs/DELIVERIES_2026-10-14.md) | Archivo de implementaciones y experimentos cerrados, con checks históricos. |
| [Historial técnico](docs/DEVELOPMENT_HISTORY.md) | Snapshot anterior a v5; archivo de decisiones y evidencia, no hoja de ruta activa. |
| [Dataset de escala](data/datasets/README.md) y [muestras](data/samples/README.md) | Fuentes, procedencia y preparación reproducible. |

Los documentos de benchmark mantienen los comandos, versiones, dispersión y contraejemplos. Sus
cifras no deben trasladarse a otra versión, navegador, dataset o equipo sin una comparación válida.

## Desarrollo

Comprobaciones habituales del workspace (requieren dependencias ya disponibles para `--offline`):

```sh
cargo test-fast --offline
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --offline -- -D warnings
cargo clippy -p viewer_wasm --target wasm32-unknown-unknown --offline -- -D warnings
node --test examples/web-host/tests/*.test.mjs
python3 -m unittest discover -s examples/web-host/tests -p 'test_*.py'
python3 -m unittest discover -s crates/viewer_convert/benchmarks -p 'test_*.py'
```

Las pruebas GPU ignoradas requieren un adaptador compatible o Vulkan por software. Para cambios en
el host web, además de las suites anteriores, comprobar con Playwright los flujos afectados, la
consola y escritorio/móvil cuando corresponda. No considerar tests nativos ni lecturas GPU como
evidencia de presentación visual en todos los navegadores.

Para compilar WASM y generar bindings/paquete local, seguir el
[host web](examples/web-host/README.md). Los perfiles `fast`, `perf` y `release` tienen objetivos
distintos; registrar el perfil al comparar medidas. El inventario inicial no reejecutó
suites ni benchmarks; la [migración posterior](docs/RENDERER_DEPENDENCIES.md) registra los checks
actuales y sus límites. Las mediciones de rendimiento anteriores pertenecen a sus revisiones.
