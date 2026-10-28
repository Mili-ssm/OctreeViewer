# Entregas y experimentos — 14-10-2026

Archivo de las entregas que figuraban en el README antes del inventario del 14-10-2026.
Se conservan sus resultados, checks y contraejemplos tal como se documentaron. Las casillas
pendientes describen el momento de cada entrega: algunas se resolvieron en las siguientes.
El seguimiento activo está únicamente en el [README](../../../README.md); el
[inventario](../../reference/inventory.md) distingue producto, prototipos y propuestas.
Los checks históricos no certifican el checkout actual ni cambios posteriores de dependencias.

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
estimadas de estructuras nativas, no tamaños de archivo ni RSS. [Contrato y evidencia](../benchmarks/QUANTIZED_OCCUPANCY_BENCH.md).

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
[Streams, archivos y límites](../benchmarks/STORAGE_BENCH.md). Pasan 315 tests nativos (3 GPU ignorados),
fmt/Clippy nativo/WASM, suites del host/runner y Blob/OPFS con Playwright MCP. Sin presentación GPU.

La comparación repetida pre2/pre3 ya está en el [informe pre3](../benchmarks/STORAGE_V6_PRE3_BENCH.md).
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
  [informe](../benchmarks/ATTRIBUTE_DICTIONARY_BENCH.md). El experimento aislado precede a la integración pre3.
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
  24 diccionarios y 96 constantes. [Informe, límites y dependencias](../benchmarks/STORAGE_V6_PRE3_BENCH.md).
- [ ] Medir RSS y caché fría/remota; extender corpus y seguir las paletas RGB/topología pendientes.

## Resúmenes por nodo y diccionario conjunto columnar — 14-10-2026

Contrato acordado y primera implementación experimental en
[resúmenes y diccionario columnar](../../explanation/NODE_ATTRIBUTE_SUMMARIES.md): los nodos internos guardan
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

## Entrega: v6.4-pre, I/O independiente y topología compacta — 14-10-2026

Prioridad acordada: reducir tamaño contando costes de escritura/lectura y consultas parciales;
el layout conjunto no es un requisito. HDD/remoto son objetivos de medición, no resultados
extrapolados desde NVMe caliente. Primera revisión: máscaras por nodo interno con hojas/prefijos
implícitos y transposición reversible de bytes por columna antes de LZ4, manteniendo los
candidatos pre3 y el reemplazo solo cuando reduce bytes almacenados.

- [x] Validar enteros/filas exactos, 33 particiones, corrupción y límites; conservar pre3 y
  anteriores, identidad explícita pre4 y rechazo de pre5/final v6.
- [x] Integrar conversión, EPT, checkpoints y lectura selectiva; comprobar encoder WASM/OPFS,
  metadata Blob y reconstrucción nativa de esas parejas. No acredita presentación GPU.
- [x] Medir parejas válidas pre3/pre4: tres rondas alternadas en cuatro muestras, hashes y
  checksums estables, tiempos y RSS por proceso. [Informe y límites](../benchmarks/STORAGE_V6_PRE4_BENCH.md).
  Belvedere: `.oct` −487.696 B y `.octattrs` −14.561.847 B; pareja −1,06 %. Outer Banks −16,49 %,
  Bunny sin ahorro. Mediana residente Belvedere 9,602 → 9,203 s, en NVMe caliente.
- [ ] Próximo cierre: medir por separado XYZ/RGB/atributo, planos de coordenadas e índice
  exterior; después grupos RGB exactos y selector de candidatos más barato. Incluir CPU/RSS,
  bytes por consulta y corpus adicional antes de otra revisión. HDD/remoto siguen sin medir.

## Medición cerrada: Albuera y Majadas, pre3 frente a pre4 — 14-10-2026

- [x] Convertir completas ambas fuentes SensOFOREST con pre3/pre4, una ejecución por caso,
  conservando las parejas y hashes. Un worker, división 2, 262.144 puntos/bloque, perfil `perf`.
- [x] Tres rondas de lectura alternadas por revisión: jerarquía, BFS completa en streaming,
  solo XYZ y clasificación. Verificar filas/CRC y checksum BFS entre revisiones; medir RSS.
- [x] Comparar tamaños lógicos y bloques asignados por Btrfs, sin equipararlos a espacio remoto.
  La lectura residente completa se excluye para evitar presión de RAM; el estado de caché no
  se controla. Datos y límites publicados, sin tratar una conversión como mediana.

Majadas: pareja −13,62 %, BFS 40,948 → 38,684 s; Albuera: −19,15 %, BFS 68,628 → 59,790 s
(medianas de tres lecturas). Conversión observada +2,32 % / +4,81 %, N=1; NVMe con caché
sin controlar. [Informe y evidencia](../benchmarks/SENSOFOREST_V6_BENCH.md).


## Entrega: concurrencia de I/O para V6 — 14-10-2026

- [x] Añadir `--pipeline-writes` optativo para LAS/LAZ: preparar el siguiente lote mientras
  se escribe el actual, dos lotes como máximo y anchura ajustada al presupuesto estimado.
  Mantiene append ordenado, revisión física y validación antes de publicar.
- [x] Verificar bytes idénticos pre1–pre4, orden/solapamiento, errores y cancelación.
- [x] Comparar tres rondas alternadas de conversión pre4 con 4 workers, pipeline apagado/
  encendido a 512 MiB y encendido a 1.024 MiB; hashes idénticos, tiempos y RSS reales.
  Belvedere: total mediano 42,681 → 41,959 s (−1,7 %, rangos solapados); la opción sigue
  apagada por defecto. Más presupuesto no aumentó concurrencia con 4 workers solicitados.
- [x] Medir lectura BFS en `storage_bench --workers 1|2|4`, con lectores persistentes y
  presupuesto simultáneo: Belvedere 11,131 → 6,918 s (−37,8 %), RSS 20,5 → 46,2 MiB;
  tres rondas por caso y checksum idéntico. Es el ejemplo nativo, no la carga del viewer.
- [ ] Medir acceso selectivo/HDD/remoto y definir contrato de lotes/cancelación antes de
  incorporar lectura paralela al viewer. La validación del conversor ya lee en paralelo.
- [ ] Retomar selector de codecs más barato y grupos RGB exactos solo tras perfiles por
  payload; criterios y contraejemplos en [el informe de I/O](../benchmarks/STORAGE_V6_IO.md).


## Entrega: reducir CPU de codecs y checksums — 14-10-2026

- [x] Sustituir IDs de diccionario bit a bit por acumuladores de bytes, conservando el stream
  LSB-first y las validaciones de padding/IDs/límites. Oracle independiente para 0–32 bits.
- [x] Comparar tres rondas alternadas del binario anterior/actual en Belvedere: conversión
  con pipeline y 4 workers; BFS con 1/4 lectores, SHA-256 de la pareja, checksum, CPU y RSS.
  Mediana de conversión 42,822 → 40,598 s (−5,2 %, rangos solapados), BFS serial
  11,242 → 10,545 s (−6,2 %). BFS con 4 lectores no mejora de forma clara.
  [Evidencia, criterios y límites](../benchmarks/STORAGE_V6_CPU.md).
- [x] Evaluar CRC IEEE por grupos de 8 bytes: mantener polinomio, resultado incremental y
  checksums antes de expandir. Medido aparte, sin cambiar revisión física ni dependencias.
  Belvedere: BFS serial 10,529 → 8,951 s (−15,0 %), conversión 41,661 → 40,399 s
  (−3,0 %, rangos solapados). Tres rondas por caso, parejas/filas idénticas; no sumar los
  porcentajes de ambas series. CRC aislado 4,44× no equivale al speedup del formato.
- [ ] Evaluar tabla directa para columnas u8 y transposición por tiles; ampliar lectura
  selectiva paralela. Mantener comparación de bytes, CPU/RSS y corpus de alta entropía.


## Entrega: diccionarios de un byte — 14-10-2026

- [x] Tabla directa de 256 entradas, preservando primera aparición, bytes y candidatos.
  Comparar todas las cardinalidades con HashMap, roundtrip, límites y corrupción.
- [x] Cerrar tres rondas por corpus tras conservar asignaciones/vida del HashMap en columnas
  anchas. Paleta/IDs aislados más de 10×; conversión final de Albuera 2,193 → 2,176 s (rangos
  solapados, sin mejora global demostrada). Belvedere es control negativo por columnas
  constantes; doce conversiones finales con parejas idénticas dentro de cada corpus. [Evidencia y prototipo descartado](../benchmarks/STORAGE_V6_CPU.md).
- [x] Cerrar experimento column/row/tile256, 432 transformaciones verificadas: tiling empeora
  los bloques habituales de 262.144 filas. Se conserva el codec actual; el único caso favorable
  (16 bytes, 1.048.576 filas) supera el límite del conversor. [Informe](../benchmarks/STORAGE_V6_BYTE_PLANES.md).
- [ ] Ampliar benchmark de lectura selectiva paralela XYZ/RGB/columna y medir transporte antes
  de integrar concurrencia/prefetch en consumidores. Retomar tiles solo con un caso real medido.
