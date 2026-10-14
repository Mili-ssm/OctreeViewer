# Octree Viewer

Visor de nubes LiDAR en Rust, WebAssembly y WebGPU. Convierte LAS/LAZ o EPT local a una
jerarquía espacial persistente y carga los bloques necesarios para la vista. El objetivo a largo
plazo es explorar fuentes mayores de **500 GB decimales** con memoria acotada. La escala completa
y cualquier superioridad frente a Potree siguen sin demostrarse.

## Estado actual — 14-10-2026

- El formato de trabajo vigente es **v5 definitivo** (`OCTVFRM1`, revisión 0), separado en `.oct` (coordenadas y jerarquía) y
  `.octattrs` (RGB y atributos numéricos). Se lee y escribe v5; hay que regenerar archivos v1–v4 y v5 provisionales (`OCTVWFRM`) desde sus fuentes.
- Están implementados **`v6.1-pre`, `v6.2-pre` y `v6.3-pre` optativos**. Pre1 añade deltas; pre2 serializa
  ocupación en arrays por profundidad dentro de cada bloque, con registros de 6 bytes y residuales
  exactos. Se elige el menor payload tras LZ4, con fallback raw/delta. V5 sigue predeterminado;
  pre3 añade constantes/diccionarios exactos por columna, con fallback y lectura selectiva.
  V6 final y pre4 permanecen rechazados.
- La conversión admite LAS/LAZ y EPT local. La demo web admite carga progresiva por bloques,
  color por RGB o componente numérico, cámara orbital y modo pájaro.
- La ruta LOD voxelizada usa celdas diádicas enteras sin solapamiento entre generaciones. El
  objetivo en píxeles guía la selección; no cambia arbitrariamente el tamaño físico de las celdas.
  Los huecos de la fuente no se rellenan. Este invariante todavía necesita cobertura en todos los
  consumidores de bloques.
- Las métricas de conversión, memoria, LOD y navegador son resultados de ejecuciones concretas,
  con límites de interpretación. No equivalen a límites garantizados ni a objetivos ya cumplidos.

## Prioridades pendientes

El orden siguiente es el seguimiento activo del proyecto. Los resultados cerrados y sus protocolos
se conservan en los documentos enlazados, no como tareas completadas en esta lista.

1. **Cerrar la corrección visual y la transición LOD.** Ya se corrigió el recorte al desplazar y
   rotar, y se ajustó el foco al seleccionar el modo pájaro. La corrección conservadora de LOD
   mantiene el volumen de los representantes padres, pero usa un suelo de resolución común que
   puede retrasar detalle. Falta conservar por región las celdas visibles anteriores hasta disponer
   de hijos, sin solapamientos ni huecos añadidos. Cierre: cortes mixtos Replacement/Additive,
   carga lenta o fallida, zoom y cámara; verificar con un dataset multibloque real y confirmar el
   movimiento/velocidad/rueda del modo pájaro con teclado físico.
2. **Cerrar la validación visual en navegador.** Firefox tiene cobertura funcional y capturas de
   muestra y algunas rondas de datos reales. Chrome/AMD sigue sin presentar incluso un clear WebGPU
   independiente, aunque la lectura GPU y el picking funcionen; no atribuirlo al HZB sin nueva
   evidencia. Falta un pase visual reproducible en Chrome y una matriz de interacción que incluya
   cámara, picking, cambios de atributo, viewport móvil y recuperación de errores. La evidencia
   disponible y sus limitaciones están en el [benchmark web](docs/WEB_PERFORMANCE_BENCH.md).
3. **Medir el pico conjunto de memoria antes de retener estado incremental.** Ya hay modelos del
   builder, muestreo RSS nativo y mediciones parciales de PSS de Firefox. No describen todavía el
   pico simultáneo del caché decodificado, acumulador/proyección, instancias, escena anterior y
   recursos GPU/WASM. Extender esa observación a la carga progresiva y a un corpus Additive real;
   después decidir si la reutilización incremental aporta suficiente trabajo ahorrado para su coste
   de memoria y complejidad. La medición estricta de prefijos disponible no justifica por sí sola
   un acumulador persistente.
4. **Completar la base de conversión, lectura y escala v5.** Las conversiones nativas repetidas de
   Belvedere, Majadas y Albuera están medidas; la matriz de navegador sigue parcial y Belvedere a
   4 px no completa el objetivo bajo el presupuesto de caché probado. Separar apertura, conversión,
   primera vista, refinamiento, RSS/PSS, temporales, bytes de archivo e I/O; declarar equipo,
   perfil y estado de caché. El objetivo de más de 500 GB y la comparación con Potree requieren una
   prueba reproducible con hardware, calidad y tareas equivalentes.
5. **Resolver requisitos de producto y soporte que aún condicionan el alcance.** Acordar plataformas
   y mínimos de RAM/GPU; dimensiones LAS/LAZ que deben preservarse (incluidos CRS, flags y Extra
   Bytes); licencia y política pública de compatibilidad; y alcance de medición, clasificación,
   recorte y exportación. Hasta acordarlos, las omisiones deben seguir siendo explícitas y los
   umbrales de conversión/tamaño son referencias de evaluación, no promesas de producto.
6. **Continuar v6 por revisiones verificables.** La primera entrega `v6.1-pre` ya lee/escribe
   coordenadas delta o directas, conserva filas/atributos/precisión de v5 y funciona con LAS/LAZ,
   EPT, checkpoints y selección programática WASM. La comparación repetida en tres muestras
   pequeñas reduce tamaño conjunto un 10,4 %, 9,8 % y 49,0 %, con costes de CPU/lectura variables;
   no demuestra rendimiento a escala. [Especificación](crates/viewer_format/FORMAT_V6_PRE1.md),
   [contratos y pendientes](docs/STORAGE_V6_PREPARATION.md) y [evidencia](docs/STORAGE_BENCH.md).
   La serie alternada del 14-10-2026 cierra tres conversiones y lecturas por formato en Belvedere:
   v5/v6 ocupan 1.591.650.185/1.591.563.835 B; conversión mediana 46,505/48,046 s y lectura
   residente 7,413/7,388 s, con caché caliente en NVMe local. Sólo ahorra 86.350 B (0,0054 %),
   sin mejora general de velocidad. Falta extender la comparación a Majadas/Albuera y medir
   memoria conjunta. Ya existe un prototipo lógico aislado
   `OccupiedOctree` en `viewer_core`: arrays de celdas ocupadas por nivel, máscara e índice
   al primer hijo, conteo derivado y coordenadas reconstruidas desde una raíz entera explícita.
   `OccupiedPointBlock` ya asocia hojas con todas las filas y atributos de un bloque y permite
   seleccionarlos por celda. `QuantizedOccupancy` reconstruye los u32 con residuales empaquetados,
   y ahora se serializa mediante el codec físico `v6.2-pre`, sin reconstruir ese índice nativo al
   leer. La memoria del prototipo no decide el layout: se priorizan bytes/I/O y CPU de reconstrucción.
   [Especificación pre2](crates/viewer_format/FORMAT_V6_PRE2.md). El piloto Belvedere pre2 ocupa
   1.590.761.099 B, 802.736 B menos que pre1 (0,0504 %), con 80 bloques de ocupación. Es una
   conversión por formato y tres lecturas BFS; la serie repetida pre2/pre3 siguiente amplía ese piloto.
   El experimento de [diccionarios de atributos](docs/ATTRIBUTE_DICTIONARY_BENCH.md) reduce los
   streams de Belvedere un 28,00 % por columna y un 44,03 % por combinación completa, contando
   cabeceras. Esta última acopla las lecturas y aumenta tamaño en dos muestras: se priorizaron
   codecs independientes. Las paletas RGB compartidas siguen pendientes.
   Ya están integrados en [pre3](docs/STORAGE_V6_PRE3_BENCH.md): la pareja Belvedere baja
   un 10,78 % con filas exactas y consultas independientes. Conversión y lectura caliente son
   más lentas; faltan caché fría/remota, RSS y más corpus.
   Su contrato local no resuelve grilla física ni CRS entre nubes; ver las tareas siguientes.
   La versión del formato en disco, la versión de esa representación lógica y la versión/algoritmos
   del renderer son contratos distintos. Caras expuestas/agrupadas y transiciones entre LOD son
   trabajo del renderer; deduplicar valores o streams es una optimización del almacenamiento.
   Medir picos de caché/render y validar WebGPU siguen pendientes. Cada cambio incompatible de
   almacenamiento requiere otra revisión pre; no habilitar `v6` final antes de cerrar compatibilidad
   y calidad.
7. **Definir el marco para varias nubes compatibles.** Es un requisito futuro que condiciona la
   topología y metadatos: CRS, unidades, ejes, origen/ancla y escala deben compartir una grilla
   diádica reproducible. Falta decidir si una escena exige una celda global fija o permite rejillas
   por nube con transformación explícita, y cómo rechazar CRS/unidades incompatibles. Resolver
   cuantización, negativos, límites enteros y escalas distintas sin desplazar silenciosamente puntos
   ni depender del orden de carga. Probar nubes convertidas por separado antes de serializar el
   contrato en otra revisión.

## Uso y formato

Conversión nativa:

```sh
cargo run --profile perf -p viewer_convert -- INPUT.laz OUTPUT.oct --format-version v5
# También admite un directorio EPT local laszip o su ept.json.
# --checkpoint conserva fases completas; --resume retoma la misma entrada/opciones.
```

La conversión produce una pareja v5 definitiva. Las muestras versionadas se han regenerado;
los datasets locales provisionales deben reconvertirse. Para ensayar las revisiones experimentales,
usar `--format-version v6.1-pre`, `--format-version v6.2-pre` o `--format-version v6.3-pre`
en lugar de `v5`. Se rechazan las versiones sin backend antes
de escribir y los manifiestos de checkpoint anteriores a V5. `v6.N-pre` identifica revisiones
exactas de desarrollo; se implementan pre1/pre2/pre3, no `v6` final.

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
| `viewer_format` | V5 y v6 pre1/pre2/pre3, cuantización, codecs, checksums y lectura selectiva. | XYZ lógico ocupa 12 bytes/punto; v6 puede reducir el stream antes de LZ4; los enteros de 64 bits conservan bits en disco, pero su proyección de color puede perder precisión. |
| `viewer_convert` | Conversión LAS/LAZ con spools/checkpoints y EPT local. | EPT remoto, binary/zstandard y checkpoints EPT no están soportados. |
| `viewer_render_wgpu` | Cámara, voxelización, buffers GPU, HZB y presentación. | Los presupuestos propios no representan toda la VRAM; la GPU puede reservar recursos fuera del control del renderer. |
| `viewer_wasm` | Worker, carga LOD y conversión web en dos pasadas. | Lecturas por bloque síncronas; convertir fuentes de cientos de GB en navegador no es requisito. |
| `examples/web-host` | Demo, API embebible, carga y descarga de archivos. | OPFS/cuota, pérdida de GPU, picking y cancelación requieren validación real en navegadores objetivo. |

La conversión conserva intensidad, clasificación, retornos, ángulo, user data y point source ID; si
existen, GPS time, RGB16 y NIR. Flags LAS, Extra Bytes y CRS requieren trabajo adicional y las
omisiones se notifican. El `.oct` conserva marco local `f64` y error de cuantización acotado; el
render reconstruye posiciones locales `f32`.

## Documentación

| Documento | Contenido vigente |
| --- | --- |
| [Preparación v6](docs/STORAGE_V6_PREPARATION.md) y [pre1](crates/viewer_format/FORMAT_V6_PRE1.md) / [pre2](crates/viewer_format/FORMAT_V6_PRE2.md) / [pre3](crates/viewer_format/FORMAT_V6_PRE3.md) | Traits, formatos experimentales, compatibilidad y criterios pendientes. |
| [Benchmarks de almacenamiento](docs/STORAGE_BENCH.md) | Suite repetida: tamaños, escritura, reconstrucción y carga secuencial de hijos. |
| [Ocupación cuantizada](docs/QUANTIZED_OCCUPANCY_BENCH.md) | Residuales exactos, coste del índice en memoria y límites del candidato. |
| [Benchmark v5 de memoria y lectura](docs/V5_MEMORY_BENCH.md) | Conversión, acceso a bloques, memoria y perfiles nativos. |
| [Benchmark de construcción de vóxeles](docs/VOXEL_BUILD_BENCH.md) | Construcción, memoria conjunta, acumuladores y experimentos de reutilización. |
| [Benchmark web](docs/WEB_PERFORMANCE_BENCH.md) | Métricas de navegador, flujos, resultados y limitaciones visuales. |
| [Selección LOD](docs/LOD_FRONTIER_BENCH.md) | Contratos y mediciones de selección de frontera y prioridad incremental. |
| [Datos reales](docs/REAL_DATASET_BENCH.md) | Protocolo y resultados históricos; revisar fecha/versión antes de comparar con v5. |
| [Ramificación espacial](docs/SPATIAL_BRANCHING_BENCH.md) | Experimentos de subdivisión y cortes con diferentes ramas. |
| [Anchura de coordenadas](docs/COORDINATE_WIDTH_BENCH.md) | Evaluación del experimento `u16` y sus límites. |
| [Historial técnico](docs/DEVELOPMENT_HISTORY.md) | Snapshot anterior a v5; archivo de decisiones y evidencia, no hoja de ruta activa. |
| [Dataset de escala](data/datasets/README.md) y [muestras](data/samples/README.md) | Fuentes, procedencia y preparación reproducible. |

Los documentos de benchmark mantienen los comandos, versiones, dispersión y contraejemplos. Sus
cifras no deben trasladarse a otra versión, navegador, dataset o equipo sin una comparación válida.

## Desarrollo

Comprobaciones habituales del workspace:

```sh
cargo test-fast --offline
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --offline -- -D warnings
cargo clippy -p viewer_wasm --target wasm32-unknown-unknown --offline -- -D warnings
node --test examples/web-host/tests/*.test.mjs
python3 -m unittest discover -s examples/web-host/tests -p 'test_*.py'
```

Las pruebas GPU ignoradas requieren un adaptador compatible o Vulkan por software. Para cambios en
el host web, además de las suites anteriores, comprobar con Playwright los flujos afectados, la
consola y escritorio/móvil cuando corresponda. No considerar tests nativos ni lecturas GPU como
evidencia de presentación visual en todos los navegadores.

## Entrega 14-10-2026: topología lógica candidata para v6

Assessment: `v6.1-pre` conserva puntos y atributos, pero delta-varint no aporta ahorro
material en el piloto Belvedere. Eliminar coordenadas de celdas hijas requiere una topología
explícita; `SpatialHierarchy` no garantiza que cada bloque sea una celda uniforme.

Plan y criterios de cierre de esta entrega:

- [x] Implementar en `viewer_core` un octree ocupado experimental, independiente del formato,
  con raíz explícita en coordenadas enteras, profundidad 0–32 y celdas hoja unitarias.
- [x] Guardar arrays por nivel, hijos contiguos en orden de octante `x<<2 | y<<1 | z`, máscara
  de ocho bits e índice al primer hijo. Derivar el conteo con popcount, sin flags indefinidos
  ni coordenadas por nodo. Mantener privados los registros para preservar invariantes.
- [x] Construir de forma determinista, colapsar entradas duplicadas, rechazar celdas fuera de
  raíz y raíces desbordadas. Permitir consulta de ocupación y reconstrucción exacta de hojas.
- [x] Probar vacío, profundidad cero, ocho octantes, negativos, límites i32, orden de entrada,
  duplicados y equivalencia con un conjunto de celdas. Pasan `cargo test-fast --offline`,
  `cargo fmt --all -- --check`, Clippy workspace/all-targets y Clippy WASM con `-D warnings`.
- [x] Actualizar contratos/documentación y revisar el diff para el commit de esta entrega.

La asociación de payloads/atributos en memoria se cierra en la entrega siguiente. Pendiente:
integración con conversión/render y semánticas Replacement/Additive; definir grilla física,
CRS/unidades y compatibilidad entre nubes; decidir conteo/flags y ancho de
índices en disco; medir tamaño total, construcción/apertura, consultas y pico de memoria frente
a v5/pre1 con datasets reales. Sólo entonces considerar otra revisión de almacenamiento.
Sugerencia: evaluar primero el conteo derivado y posponer flags de deduplicación hasta disponer
de patrones medidos. Este prototipo no cambia conversión, renderer ni compatibilidad en disco,
y no demuestra ahorro, velocidad o menor RSS.

## Entrega 14-10-2026: asociar ocupación y filas de puntos

Assessment: la topología ya reconstruye celdas, pero colapsar celdas duplicadas no puede
colapsar filas de puntos ni sus atributos. Una máscara no sustituye XYZ dentro de la celda.
Esta entrega enlaza hojas con filas originales y mantiene el payload íntegro en memoria.

- [x] Añadir consulta de índice de hoja al octree y reutilizar un único constructor de
  topología desde claves ordenadas, sin duplicar el algoritmo ni ordenar dos veces.
- [x] Asociar cada celda hoja con un slice contiguo de índices de filas, conservando todos
  los duplicados como filas diferentes y el orden original dentro de cada celda.
- [x] Vincular el índice a un `PointBlock` inmutable; validar una celda por punto y la
  alineación de las columnas. Permitir seleccionar una celda con XYZ/RGB/atributos completos.
- [x] Probar selección por celda contra un agrupado independiente, vacíos, límites,
  multiplicidad y conservación de bits (NaN, cero negativo y enteros de 64 bits). Comprobar
  también bloques leídos desde parejas v5/v6.1-pre. Pasan tests del workspace, fmt y
  Clippy nativo/WASM con `-D warnings`; las pruebas GPU ignoradas no certifican presentación.
- [x] Actualizar documentación y revisar el diff para el commit de esta entrega.

Pendiente: grilla física, residuales XYZ para comprimir sin pérdida, asociación de payloads en
disco, consultas selectivas, Replacement/Additive y mediciones de tamaño/CPU/pico conjunto.
No se habilita una revisión nueva de archivo ni se cambia el renderer en esta entrega.

Próximos cierres de v6, en orden de dependencia:

- [ ] Definir y probar la grilla física candidata (ancla/escala diádica, raíz y límites),
  con negativos y nubes independientes. Explicitar CRS/unidades incompatibles y comprobar
  que el resultado no dependa del orden de puntos ni de carga de las nubes.
- [x] Probar residuales XYZ sobre los u32 originales: candidato exacto con 0–32 bits residuales,
  proyección/error conservados y asociación de RGB/atributos por fila. La serialización por
  bloque queda implementada en pre2; véase la entrega siguiente.
- [ ] Medir ocupación + IDs/offsets + payload completo en corpus real y comparar con v5/pre1:
  tamaño total, construcción, selección por celda, CPU y pico simultáneo. Conservar como
  contraejemplo el ahorro insignificante de delta-varint en Belvedere.
- [x] Fijar pre2 con registros de 6 bytes, límites, checksums, identidad exacta y lectura selectiva
  por bloque/columna. El índice exterior y un único array global por profundidad siguen pendientes.

## Entrega 14-10-2026: ocupación y residuales exactos de coordenadas cuantizadas

Assessment: el benchmark de Belvedere no muestra ahorro material con delta-varint. La
cuantización vigente es afín por eje/bloque, con escalas distintas o cero: una celda de sus
enteros no equivale necesariamente a un cubo físico. El codec de esos enteros puede investigarse
sin imponer una grilla física ni inventar compatibilidad CRS/unidades todavía ausente.

- [x] Extraer un índice celda→filas compartido, ahora que tiene dos consumidores reales:
  el `PointBlock` existente y el candidato de coordenadas cuantizadas.
- [x] Separar cada u32 en parte alta/celda y residual bajo de 0–32 bits, empaquetado en bytes;
  conservar transformación/error y todas las filas, sin conservar además el vector XYZ original.
- [x] Reconstruir exactamente los u32 y reutilizar su decodificación original. Probar extremos,
  duplicados, ejes constantes, vacío, alta entropía, orden/atributos y particiones 0/1/16/31/32.
- [x] Añadir un benchmark acotado desde LAS/LAZ antes de f32: construcción/reconstrucción,
  filas/hojas/nodos y bytes de capacidad retenida; registrar muestras, hashes y límites.
- [x] Validar regresión (309 tests pasan, 3 GPU ignorados), fmt y Clippy nativo/WASM;
  actualizar documentación y preparar el commit de esta entrega.

Esta entrega no cambia el formato en disco. La raíz de este candidato representa código entero
local al bloque, no un CRS ni grilla compartida. Medir bytes retenidos no equivale a medir un
archivo comprimido ni RSS. Después: evaluar un codec completo con límites/lectura selectiva y
mediciones de streams materializados antes de asignar otra revisión pre; la grilla física común
sigue siendo una línea de integración independiente.


Resultado: 360 muestras sobre prefijos de 32768 puntos de Outer Banks, Belvedere, Majadas y
Albuera, con reconstrucción exacta en todos los casos. La mejor partición medida retiene
1,42–1,59× el vector cuantizado original; la topología profunda llega a ~36×. Son capacidades
estimadas de estructuras nativas, no tamaños de archivo ni RSS. [Contrato y evidencia](docs/QUANTIZED_OCCUPANCY_BENCH.md).

- [x] Serializar máscaras/conteo u8 e índices u32 por nivel en pre2, con filas en varints.
  Las estructuras en memoria se podrán optimizar por rendimiento independientemente del disco.
- [x] Materializar y validar topología + filas + residuales + metadata y compararlos tras LZ4
  con raw/delta. Integrado en pre2; el criterio es tamaño físico y coste de lectura/conversión.

## Entrega 14-10-2026: representación física v6.2-pre

Objetivo corregido: reducir bytes almacenados/I/O, aceptando CPU de reconstrucción. La memoria
nativa del prototipo no decide la viabilidad del codec. Se conserva precisión y asociación de filas.

- [x] Serializar arrays por profundidad dentro de cada bloque direccionable: máscara u8,
  conteo u8 e índice u32 al primer hijo (6 bytes por registro, sin padding Rust).
- [x] Comparar conteo solo frente a conteo en 4 bits + octante propio en 3 bits. Mantener el
  octante sólo cuando la variante resulte útil para compresión/lectura; documentar su señalización.
- [x] Completar el stream con filas y residuales exactos; validar niveles, índices, máscaras,
  conteos, filas únicas, varints, padding y límites antes de reconstruir.
- [x] Habilitar lectura/escritura `v6.2-pre`, conservando v5/pre1. Comparar bytes finales tras
  raw/LZ4 y mantener fallback si ocupación no reduce tamaño. Probar selección por bloque/columna.
- [x] Medir parejas reales y tiempos de escritura/lectura; actualizar README y preparar el commit.

Los arrays se segmentan por bloque para mantener I/O selectivo; no se obliga a cargar un único
array de toda la nube. La topología codifica los u32 cuantizados, no una nueva grilla física común.


Resultado del byte de conteo: inferir el octante comprime mejor en los ocho prefijos/perfiles
medidos; guardarlo mejora algunas muestras de CPU, sin ventaja general. La variante explícita
se admite pero sólo gana si reduce bytes. El caso sintético reduce pareja un 7,22 % frente a
pre1; Outer Banks/Bunny mantienen fallback. Belvedere mejora un 0,0504 % frente a pre1, con
mayor conversión observada: no hay ahorro grande ni aceleración general demostrados.
[Streams, archivos y límites](docs/STORAGE_BENCH.md). Pasan 315 tests nativos (3 GPU ignorados),
fmt/Clippy nativo/WASM, suites del host/runner y Blob/OPFS con Playwright MCP. Sin presentación GPU.

La comparación repetida pre2/pre3 ya está en el [informe pre3](docs/STORAGE_V6_PRE3_BENCH.md).
Pendientes de almacenamiento: ampliar perfiles/orden de streams y el corpus más allá de
Belvedere (Majadas/Albuera), compactar el índice exterior y estudiar un único
array global por profundidad con paginación y grilla/raíces explícitas. Medir siempre bytes finales
con metadata/atributos y lecturas equivalentes; la memoria nativa queda como optimización separada.

## Diccionarios de atributos: experimento cerrado — 14-10-2026

- [x] Confirmar que `.octattrs` no tenía diccionario; implementar benchmark de diccionarios
  por columna y por combinación completa, incluyendo RGB y conservando bits/filas.
- [x] Escribir/releer streams en tres muestras reales y una sintética, incluido Belvedere
  completo; medir tablas, IDs empaquetados, cabeceras, cardinalidad y CPU con tres repeticiones.
- [x] Documentar resultados, contraejemplos, coste de acceso selectivo y límites en el
  [informe](docs/ATTRIBUTE_DICTIONARY_BENCH.md). El experimento aislado precede a la integración pre3.
- [ ] Probar bitpacking de enteros por rango y selección de candidatos más barata;
  comparar paleta conjunta RGB8+RGB16 conservando independientes los atributos numéricos.
- [ ] Ensayar en `.oct` prefijos `first_child` reconstruibles y hojas implícitas; comparar bytes
  finales tras LZ4, CPU y exactitud frente a pre2, por separado y juntos.
- [ ] Integrar sólo candidatos medidos en una revisión explícita, con CRC/límites, lectura
  selectiva, checkpoints, WASM, CPU/RSS y más corpus. Los tamaños proyectados no son archivos válidos.

## Entrega: atributos físicos v6.3-pre — 14-10-2026

- [x] Añadir codecs constante/diccionario exactos por columna, con fallback raw/LZ4 según bytes
  finales y sin acoplar columnas ni modificar v5/pre1/pre2.
- [x] Validar corrupción, límites, tipos/componentes, lectura selectiva, conversión/checkpoints y WASM.
- [x] Comparar parejas válidas y checksums con tres conversiones alternadas por revisión en cuatro
  muestras. Belvedere: 1.590.761.099 → 1.419.356.066 B (−10,78 %); conversión mediana
  62,368 → 68,923 s y lectura residente 7,640 → 9,420 s, con caché caliente.
- [x] Comprobar OPFS/Blob con Playwright y reconstruir sus payloads nativamente, incluidos
  24 diccionarios y 96 constantes. [Informe, límites y dependencias](docs/STORAGE_V6_PRE3_BENCH.md).
- [ ] Medir RSS y caché fría/remota; extender corpus y seguir las paletas RGB/topología pendientes.

## Resúmenes por nodo y diccionario conjunto columnar — 14-10-2026

Contrato acordado y primera implementación experimental en
[resúmenes y diccionario columnar](docs/NODE_ATTRIBUTE_SUMMARIES.md): los nodos internos guardan
medias ponderadas (intensidad **solo media**), presencia de categorías mediante máscaras de
ancho fijo y extremos temporales en el tipo original. Las hojas mantienen todas sus filas
exactas, incluidos duplicados. Las reglas se seleccionan explícitamente por atributo.

- [x] Implementar resúmenes alineados con profundidad/índice de `OccupiedCellIndex`, sin
  sustituir las filas originales; RGB en espacio lineal, conteos válidos por componente.
- [x] Implementar contenedor experimental por bloque: un ID compartido de combinación completa,
  tablas por columna, IDs empaquetados, CRC y lectura selectiva con acceso por profundidad/índice.
- [x] Añadir pruebas focalizadas y experimento reproducible contra columnas independientes
  raw/LZ4, separando filas originales y resúmenes internos.
- [x] Medir Belvedere completo, Outer Banks, Bunny y dos casos sintéticos con roundtrip de
  artefactos. Belvedere: −52,01 % en filas exactas frente al control raw/LZ4; +1,45 % en resúmenes.
  Consultar clasificación lee 150,16 MB frente a 0,87 MB del control por los IDs compartidos;
  este resultado no acredita una reducción equivalente de parejas válidas pre3.
- [ ] Comparar codecs pre3 dentro de una misma envoltura, medir RSS/caché fría/WASM y ensayar
  agrupaciones/fallback: el diccionario de todas las columnas no debe imponerse universalmente.
- [ ] Integrar metadatos de políticas/dominios categóricos, diccionarios de strings y validez
  explícita cuando existan en el origen; los códigos categóricos actuales son numéricos y densos.
- [ ] Diseñar la revisión física y conectar conversión/lectura/LOD/filtros, compatibilidad,
  checkpoints y navegador. El contenedor experimental no es una pareja `.oct`/`.octattrs` válida.
