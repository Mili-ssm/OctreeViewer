# Almacenamiento v5 definitivo y preparación de v6

Estado: 14-10-2026. La ruptura de v5 provisional fue autorizada antes de desarrollar v6.
Están implementados v5 definitivo y las revisiones experimentales `v6.1-pre` y `v6.2-pre`. Pre1
comprime los mismos enteros cuantizados mediante deltas/fallback, sin nueva pérdida. Su contrato
completo está en [v6.1-pre](../crates/viewer_format/FORMAT_V6_PRE1.md); una representación lógica
de vóxeles por ocupación tiene topología, asociación a filas y separación exacta de u32
en celdas/residuales. Pre2 ya serializa arrays por profundidad y registros de seis bytes dentro
de cada bloque, conservando los contratos del lector y el índice exterior. Una topología global
y grilla física común siguen pendientes. [Contrato físico pre2](../crates/viewer_format/FORMAT_V6_PRE2.md).

## Capas y versiones

Hay tres contratos distintos que no deben llamarse todos «versión v6»:

| Capa | Qué define | Identificación vigente / futura |
| --- | --- | --- |
| **Formato de almacenamiento** | Bytes de `.oct`/`.octattrs`, codecs, índices, checksums y cómo se reconstruyen sus registros. | El campo de cabecera identifica hoy v5 (`5/0`), pre1 (`6/1`) o pre2 (`6/2`). Cada cambio incompatible de bytes requiere otra revisión exacta. |
| **Representación lógica de octree/vóxeles** | Significado de nodos/celdas, grilla, ramificación, orden de hijos y cómo se derivan sus coordenadas. | Pre2 codifica ocupación en el espacio entero por bloque; un octree global de vóxeles físicos sigue pendiente. Si su contrato evoluciona de forma independiente habrá que versionarlo explícitamente en datos o metadatos. |
| **Render** | Cómo se proyectan y dibujan datos: buffers, caras, instancing, culling, cortes y transiciones LOD. | Lo determina la implementación del renderer/WASM/worker. Esas técnicas no son una capacidad ni una versión del octree almacenado, salvo que el archivo empiece a guardar datos específicos del render. |

La cabecera `FormatVersion` actual sólo versiona el formato de archivo. No existe aún un campo
`OctreeRepresentationVersion` ni un `RendererVersion` serializado. No se debe subir o cambiar la
versión del `.oct` por cambiar un shader o una transición visual que consume los mismos datos.

## Prototipo lógico de ocupación (implementado en memoria)

`viewer_core::OccupiedOctree` recibe celdas `VoxelCoord` y una `OccupiedOctreeRoot`
explícita: origen i32 y profundidad 0–32, cubo semiabierto de lado `2^depth` en unidades
abstractas de hoja. La raíz completa debe caber en el dominio i32; los cálculos intermedios
usan i64. No convierte puntos físicos, no infiere CRS y no alinea archivos independientes.

Cada nivel contiene nodos ocupados, con hijos contiguos ordenados por `(x<<2)|(y<<1)|z`,
la misma convención que `SpatialSubdivision`. Un nodo guarda máscara u8 e índice usize al
primer hijo del siguiente nivel; el conteo se deriva con popcount. Las hojas tienen máscara
e índice cero. No hay flags, payloads ni coordenadas persistentes por nodo. Los registros
son privados y sólo se exponen vistas inmutables y getters. El árbol vacío conserva
`depth+1` niveles vacíos; profundidad cero admite una única celda.

La construcción valida límites, ordena claves de camino u128 (hasta 96 bits) y elimina
duplicados. Agrupa prefijos por nivel en O(N × profundidad) después de ordenar, con O(N)
temporales más la salida; esos temporales se liberan al terminar. La consulta usa rank de
máscara; `leaf_index` devuelve el índice en el último nivel, en el orden de `cells()`.
La enumeración reconstruye hojas con estado O(profundidad). Las asignaciones de
memoria siguen la política normal de Rust, sin recuperación de OOM. Duplicados colapsados
significan ocupación, no conteo de puntos ni agregación de atributos.

Pruebas: reconstrucción/consulta contra un conjunto independiente, máscaras y rangos por
nivel, octantes completos, negativos, entrada invertida, duplicados, vacío, profundidad cero,
profundidad 32, extremos i32 y errores de límites. Este contrato todavía no se consume desde
conversión ni renderer. `usize` y el padding Rust no son una especificación binaria ni una
medición de RSS; no se atribuye ahorro de almacenamiento o velocidad al prototipo.

## Asociación de hojas y payloads (implementada en memoria)

`OccupiedPointBlock::new(root, source, cells)` recibe una celda explícita por fila del
`PointBlock` original. Mueve el payload sin clonarlo ni reordenarlo y valida tanto la cantidad
de celdas como la longitud de **cada** columna (`filas × componentes`, con comprobación de
desbordamiento). `PointBlock::new` por sí solo no impone esa alineación. Celdas fuera de raíz
y payloads desalineados devuelven errores tipados antes de construir la topología.

`OccupiedCellIndex` comparte el índice entre `OccupiedPointBlock` y el candidato cuantizado.
Los pares `(clave de camino, fila original)` se ordenan una sola vez. La construcción emite
claves únicas, `row_ids` y `leaf_offsets`: para N filas y L hojas, los offsets tienen longitud
L+1, parten de cero y terminan en N; cada slice ocupado es no vacío y todos los IDs forman
una permutación de `0..N`. Dentro de cada hoja, las filas están en orden original ascendente.
El vacío tiene offsets `[0]` y cero filas. Las hojas coinciden con el último nivel del octree.
El buffer temporal de pares se libera antes de construir la topología con claves ya ordenadas.
Los IDs se asignan en un vector propio para evitar retener capacidad de los pares más anchos.
Esto describe la vida de los buffers, no una medición de pico RSS.

`source()` y `topology()` son vistas inmutables; `into_source()` recupera el bloque original.
`rows_for_cell` y `rows_for_leaf` prestan slices de IDs sin copiar payloads. `select_cell`
copia sólo las filas seleccionadas y sus columnas mediante `AttributeValues::select_rows`,
conservando XYZ/RGB, descriptores, componentes y bits de valores tipados. Una celda ausente o
fuera de raíz devuelve `Ok(None)`. El bloque seleccionado mantiene el ID de origen como
procedencia: es un subconjunto y no debe sustituir al bloque completo en jerarquía/caché.

La asociación es estructural y local a ese bloque. El caller suministra las celdas; no se
comprueba que el XYZ físico esté dentro de ellas porque aún no hay contrato de grilla física.
No se deduplican puntos ni se promedian valores. Conservar el `PointBlock` no equivale a
conservar LAS original sin pérdida: el lector ya aplica su cuantización/proyección. Tampoco
permite quitar XYZ del payload: faltan residuales exactos y su codificación.

Pruebas nuevas: agrupado independiente de 300 filas, cobertura de cada fila una sola vez,
multiplicidad, límites y errores, RGB distintos dentro de una celda, atributos multicomponente,
enteros i64/u64 extremos y superiores a 2^53, NaN con payload y cero negativo comparados por
bits. La integración lee parejas v5 y v6.1-pre con/sin acompañante y comprueba selección por
celda. Son fixtures sintéticas, no evidencia de escala ni rendimiento en datasets reales.

Siguiente contrato pendiente: grilla física/CRS, codec completo de residuales XYZ, identidad de filas entre
bloques, payloads Replacement/Additive y anchos de IDs/offsets serializados. Antes de otra
revisión pre, medir bytes totales, CPU y picos conjuntos con payload e índice simultáneos.

## Residuales exactos en el espacio cuantizado (implementados en memoria)

`QuantizedOccupancy::new(&QuantizedCoordinates, residual_bits)` recibe los u32 originales,
antes de cualquier proyección f32. Para `b` entre 0 y 32 divide `q` en `coarse = q >> b` y
`residual = q & ((1<<b)-1)` mediante u64, incluidos los extremos. El residual se empaqueta
en `ceil(filas × 3 × b / 8)` bytes, en orden original de fila/XYZ y bits menos significativos
primero; el padding final es cero. A 32 bits el residual contiene todo XYZ y a cero no hay
bytes residuales.

La celda lógica es `i32::MIN + coarse`, con raíz `[i32::MIN;3]` y profundidad `32-b`.
`OccupiedCellIndex` conserva las filas. `reconstruct()` combina celda y residual en u64 y
restituye todos los u32 en su orden original, con transformación/error idénticos. La
proyección posterior reutiliza `QuantizedCoordinates::decode_local`, sin duplicar su fórmula.
El candidato no retiene además el vector original; el llamador puede liberarlo tras construir.

Este espacio de código no tiene significado de CRS ni volumen físico: la cuantización actual
es afín por eje/bloque y sus escalas pueden diferir o ser cero. Por eso la exactitud del codec
entero se puede investigar sin fingir una grilla física común. La metadata CRS actual sigue
siendo opcional y opaca; no hay contrato normalizado de unidades/ejes para alinear nubes.

Pasan las 33 particiones de bits, un oráculo independiente del empaquetado, negativos de la
transformación, vacío, duplicados, puntos distintos dentro de una celda, extremos y asociación
de RGB/atributos tipados. El [benchmark acotado](QUANTIZED_OCCUPANCY_BENCH.md) usa originales
LAS/LAZ antes de f32, sin requantizar lecturas del `.oct`. Los índices profundos amplifican
memoria y aun la mejor partición probada retiene más que el vector fuente. Este resultado
no mide bytes físicos ni decide el codec de disco; la serialización pre2 se evalúa aparte.

Pre2 materializa y valida el stream completo con arrays por profundidad, registros de 6 bytes,
filas en varints y residuales. Compara bytes reales tras LZ4 frente a raw/delta y conserva el menor.
El byte de conteo reserva cuatro bits para 0–8 hijos y admite tres de octante propio, señalizados
por configuración; si se infiere, esos bits son cero. Las pruebas de prefijos favorecen inferencia
para tamaño. [Resultados físicos](STORAGE_BENCH.md). La memoria nativa queda como optimización
independiente; grilla física/CRS y topología global continúan pendientes.

## Topología global y extensiones de ocupación (pendientes)

La propuesta para futuras revisiones es almacenar cada nivel como arrays densos y deterministas de
celdas ocupadas. El registro de cada padre tendría: (1) un byte como matriz/máscara de posiciones
ocupadas, (2) un byte de conteo de hijos, posiblemente con flags de compresión todavía por definir,
y (3) el índice/dirección de su primer hijo en el array siguiente. El conteo y la dirección permiten
extraer el slice contiguo de hijos; la máscara dice a qué posiciones posibles corresponden, porque
esos hijos se escriben en el mismo orden determinista que las posiciones, por ejemplo Morton. Dada
la coordenada del padre y la grilla, cada coordenada hija se deriva de su ordinal local: no se
almacena por separado. Esta derivación topológica de coordenadas es distinta del codec delta-varint
de `v6.1-pre`, que sí almacena las coordenadas cuantizadas de cada punto y sólo codifica sus
diferencias.

En un octree 2×2×2 bastaría una máscara de 8 bits por padre, con una convención fija para mapear
bits a `(x,y,z)` o a ordinales Morton. El slice es `children[first_child .. first_child + child_count]`.
Para buscar el hijo de una posición concreta, `rank(mask, bit)` cuenta los bits ocupados anteriores;
el hijo correspondiente está en `first_child + rank`. El conteo se puede contrastar con
`popcount(mask)` al validar. Guardarlo evita recalcularlo y permite tomar el slice directamente,
pero es información redundante si la máscara está disponible; el byte también podría reservar bits
para flags si se define su semántica. Por ejemplo, podrían indicar que todos los hijos son iguales,
que se repiten en pares u otro patrón compacto. Falta definir qué significa "iguales" (mismos valores,
misma ocupación, subárboles equivalentes, etc.) y cómo se expande ese patrón al reconstruir. Para
2³, el conteo 0–8 cabe en cuatro bits y deja como máximo cuatro bits de flags dentro del byte; si se
permiten ramificaciones mayores habrá que revisar ese empaquetado junto con el ancho del conteo.

Para que ese slice sea válido, los hijos de cada padre deben ser contiguos y estar en el orden que
declara la máscara. El campo `first_child` debe definirse como índice/offset dentro del array
correspondiente, no como una dirección de memoria del proceso. El ancho del conteo depende de la
ramificación: un byte alcanza para los 0–8 hijos de 2³ y para 4³, pero no para los 512 hijos posibles
de 8³. La máscara misma necesita 8, 64 o 512 bits para ramificaciones 2³, 4³ u 8³, o una
codificación dispersa equivalente. El diseño debe decidir si todas las capas son octrees 2³ o si
admiten esas subdivisiones mayores.

Si un flag comprime hijos repetidos, el conteo debe aclarar si mide hijos lógicos (`popcount(mask)`)
o registros físicos guardados en el slice. Si algunos hijos lógicos comparten una sola entrada
física, `first_child + logical_count` ya no delimita ese slice; hará falta un conteo físico o una
tabla de expansión definida. La interpretación debe permitir reconstruir sin ambigüedad la máscara
completa de hijos y su relación con las entradas físicas.

La representación también debe fijar el orden global de padres, los límites de cada array de nivel,
la coordenada/escala de la raíz, el ancho/unidad del offset y la relación entre celdas ocupadas,
hojas y payloads de puntos. El modelo actual de `SpatialHierarchy`/`PointBlock` no implica por sí
solo que sus bloques correspondan a celdas voxel uniformes.

Potencial esperado, aún no medido: quitar coordenadas repetidas de hijos y reducir bytes/metadatos
del índice; enumerar hijos ocupados mediante máscara; reconstruir cada coordenada recorriendo desde
la raíz. No elimina automáticamente los payloads de los puntos, RGB o atributos, ni acelera el
render por sí mismo. El tamaño neto debe incluir máscaras, offsets/rangos, raíces, índices y codecs;
la CPU de reconstrucción, memoria de apertura, consultas selectivas y exactitud espacial se miden
frente a v5 y `v6.1-pre`.

Las caras expuestas o agrupadas son geometría de presentación/render, no nodos del octree. Las
transiciones entre cortes LOD son lógica de selección/presentación del viewer; tampoco describen
cómo se serializan los hijos. Deben diseñarse y medirse en el renderer independientemente de la
revisión del formato y de la representación lógica, salvo que se decida almacenar una malla derivada.

## Identidad y compatibilidad

La envoltura definitiva conserva 24 bytes, con magic común **`OCTVFRM1`** (envelope 1):

| Offset | Campo | v5 definitivo | v6 |
| ---: | --- | --- | --- |
| 8 | Versión mayor `u16` LE | 5 | 6 |
| 10 | Longitud `u16` LE | 24 | 24 |
| 12 | Rol `u32` LE | 0 monolítico / 1 primario separado / 2 acompañante | mismos roles |
| 16 | Revisión de desarrollo `u32` LE | 0 obligatorio | 1, 2… desarrollo; 0 final |
| 20 | CRC32 `u32` LE | bytes 0–19 | bytes 0–19, incluida revisión |

`FileRole::{Monolithic, SplitPrimary, Attributes}` sustituye flags sueltos en la API.
`FormatIdentity::to_header_bytes` y su parser son compartidos por lector y escritor.
`FormatVersion::{V5, V6Pre(NonZeroU32), V6, Unknown { major, revision }}` distingue `v5`,
`v6.1-pre`, `v6.2-pre` y `v6`. Cada cambio incompatible durante desarrollo exige una revisión
nueva; final v5 permanece con mayor 5/revisión 0. La revisión no identifica el crate ni el commit.
No se infiere compatibilidad por orden de versión: cada backend debe admitir identidades exactas.

Los archivos provisionales con `OCTVWFRM` reciben `ProvisionalFormat`, con indicación de regenerar
LAS/LAZ/EPT. No hay lector legacy ni actualización automática de cabecera. Se han regenerado las
parejas versionadas de Outer Banks y la muestra web; los datasets locales provisionales y sus
checkpoints anteriores deben regenerarse. El paquete web local también debe reconstruirse.

`probe_format(&mut Read + Seek)` valida cabecera/CRC sin leer índice ni payload y restaura la
posición incluso si falla el análisis. No certifica el archivo completo. `FormatReader` selecciona
el backend por esa identidad y entrega la cabecera ya validada al backend indexado compartido; no la vuelve a leer.
La selección admite exactamente v5, `v6.1-pre` y `v6.2-pre`. V6 final, otras revisiones y las identidades
desconocidas fallan antes de cargar un índice.

Un hash no sustituye al campo de compatibilidad. El CRC detecta corrupción accidental; SHA-256
de fuente/binarios/salidas identifica las ejecuciones de benchmark. `v6.1-pre` usa sus
propios marcadores de índice/footer y valida identidad exacta de ambos archivos, además de su
asociación al dataset. No se admitirán parejas v5/v6 ni revisiones de desarrollo distintas.

## Interfaces implementadas y puntos de extensión

| Contrato / consumidor | Estado implementado | Pendiente para v6 |
| --- | --- | --- |
| `DatasetSource` (`viewer_core`) | Metadata y `PointBlock` independientes del layout; error asociado. | Preservar coordenadas locales, esquema y orden de filas. |
| `OctreeReader` | Acceso selectivo XYZ/RGB/columna/fila/rangos, identidad exacta y uso mediante `dyn`. | Conformidad v5/pre1/pre2 ya probada; extender al corpus grande y futuras revisiones. |
| `FormatReader<R>` | Fachada de transporte en `storage_reader.rs`, con backend privado `IndexedReader` con `StorageLayout::{V5,V6Pre1,V6Pre2}`; conserva attach, forks, metadata y estadísticas. | Añadir backend y ampliar despacho interno; los consumidores conservan la fachada pública. |
| `OctreeWriter` / `PreparedOctreeWriter` | Ingesta y cierre; bloque preparado asociado a cada escritor. | Ya escribe las tres versiones y verifica identidad del preparado; ampliar perfiles futuros. |
| `BlockEncoder` / `FormatBlockEncoder` | Encoder inmutable `Sync`, bloque asociado `Send`; preparación y capacidad retenida separadas de los sinks. | Ya selecciona v5/pre1/pre2; futuras transformaciones requieren perfiles nuevos. |
| Conversor nativo | `stream_blocks<W,E>` consume `E::PreparedBlock`; no llama `prepare_point_block` ni fija `PreparedPointBlock`. Mantiene append ordenado y liberación de spool. | Medir picos de conversión a escala y costes de candidatos simultáneos. |
| Salida LAS/LAZ, EPT y navegador | Fábricas `new_for_version` / `new_split_for_version`; rechazan versiones sin escritor antes de tocar sinks. `ConvertOptions::with_format_version` y CLI `--format-version` seleccionan explícitamente. | Hoy se escribe v5, pre1 o pre2; otras identidades siguen rechazadas. |
| WASM (`bindings.rs`, `LodSession`) | Conserva `FormatReader<BrowserBlobReader>`, que ahora es la fachada; carga selectiva por `OctreeReader`. Browser converter usa v5 por defecto y `createForVersion` permite pre1/pre2. | Blob/OPFS v6 comprobados; falta refinamiento/presentación WebGPU. |
| Checkpoints | Manifiesto V5 persiste mayor, revisión y envelope desde las opciones; rechaza identidades distintas y manifiestos V1–V4. | Reutilizar la validación exacta al habilitar nuevas identidades. |
| `SpatialHierarchy` | Jerarquía lógica común y `child_ids` público para recorrido estructural independiente del orden físico. | Preservar Replacement/Additive, conteos y grilla común. |

Lectura y escritura seleccionan ahora el perfil exacto del layout. Los perfiles comparten el
pipeline de índices, jerarquía, atributos, CRC y transportes; no duplican el parser v5. Los bloques
preparados registran su perfil, y un escritor de otro formato los rechaza antes de I/O. No se exige
`Sync` al escritor: los workers comparten sólo el encoder inmutable y escriben después en orden.

La intercambiabilidad es lógica, no mezcla de bytes ni de acompañantes. Si v6 introduce topología
voxelizada sin puntos equivalentes, primero debe resolverse su representación en el contrato común;
un enum de versiones no resuelve esa diferencia de datos.

## Validación y cierre pendiente

Las pruebas cubren cabecera/roles/CRC, provisional y versiones sin backend, roundtrip monolítico y
split, selección real de I/O, forks, acompañantes incorrectos, preparación serial/paralela y
checkpoints incompatibles. La suite formal incluye comparación alternada v5/v6.1-pre en tres muestras reales; véase
[el protocolo y evidencia](STORAGE_BENCH.md).

Playwright MCP comprobó Blob con/sin acompañante, rechazo de provisional y `v6.2-pre`,
acompañante incorrecto y conversión OPFS seleccionando v6.1-pre. La pareja OPFS v6 se descargó
y reconstruyó con el lector nativo, con el mismo checksum lógico que la conversión web v5. Su navegador no
ofreció adaptador WebGPU: estas comprobaciones no certifican presentación ni refinamiento GPU.

Un piloto nativo en Belvedere (81.682.329 puntos fuente, 1.343 bloques) comprobó compatibilidad
de salida y lectura BFS en ambas versiones. La pareja completa sólo bajó 86.350 B (0,0054 %) con
v6.1-pre; las medianas de carga progresiva caliente fueron 6.706 ms (v5) y 6.719 ms (v6), con el
mismo checksum. Es una conversión por versión y tres lecturas por pareja, no una comparación formal
repetida de conversiones ni una medición de cámara/GPU. El informe y hashes están en
[`storage-v6-pre1-belvedere-pilot-2026-10-13.json`](benchmarks/storage-v6-pre1-belvedere-pilot-2026-10-13.json).

La [serie repetida del 14-10-2026](benchmarks/storage-v5-v6-pre1-comparison-2026-10-14.json)
cierra las tres conversiones alternadas por formato en Belvedere, además de dos muestras pequeñas.
Con NVMe local y caché caliente, Belvedere conserva el ahorro de 86.350 B (0,0054 %); conversión
mediana v5/v6 46,505/48,046 s, carga residente 7,413/7,388 s y BFS 6,710/6,678 s. Conteos y
checksums coinciden; no hay una mejora general de velocidad. No incluye RSS ni el prototipo de
ocupación, que todavía no se serializa. Rangos y opciones en [el informe](STORAGE_BENCH.md).

### Decisiones abiertas de grilla común

El formato actual cuantiza por bloque y guarda marco local por dataset; no se debe interpretar como
una grilla común entre archivos. Antes de añadir ocupación/topología al formato hay que fijar:

- Si una escena elige una celda diádica común al abrirla o cada dataset guarda su celda y transforma
  coordenadas al marco de escena.
- Qué CRS, unidades y orden de ejes son comparables; CRS desconocido o distintos deben rechazarse
  explícitamente hasta existir una transformación definida.
- Cómo se serializan ancla, origen negativo, escala y límites con enteros firmados, y qué error se
  permite al convertir coordenadas LAS/EPT y reconstruirlas en el renderer.
- Si las celdas ocupadas representan puntos de origen, centros cuantizados o volumen cubierto; cómo
  se relacionan esos registros con `PointBlock`, Replacement/Additive y los representantes LOD.

El contrato candidato debe probar nubes convertidas por separado, en orden de carga invertido,
incluyendo límites de celda, negativos, escalas diferentes y CRS/unidades incompatibles. Hasta
resolverlo no se serializa una `GridDefinition` ni se asigna una revisión nueva.

Para cerrar v6 faltan:

- Extender perfiles/orden de streams e índice exterior de pre2, ya integrado y con índices u32
  y byte conteo/octante definido. Evaluar topología global conservando paginación; medir tamaño
  total, CPU, RAM, lectura selectiva y equivalencia espacial antes de fijar una revisión de
  almacenamiento.
- Evaluar deduplicación de valores o streams de atributos como compresión del almacenamiento, con
  acceso selectivo y asociaciones de filas conservadas.
- Evaluar caras expuestas/agrupadas y transiciones entre cortes LOD como características separadas
  del renderer; sólo versionar el archivo si se decide persistir una representación de malla derivada.
- Extender la conformidad actual a cambios nuevos y al corpus grande; medir límites/picos
  simultáneos de conversión, caché, acumulador e instancias.
- Extender a Majadas/Albuera la comparación formal ya completada en Belvedere el 14-10-2026
  (tres conversiones por formato, orden alternado, lecturas/checksums equivalentes). Completar
  memoria y mantener calidad/atributos equivalentes. Belvedere sigue sin ahorro material.
- Validación de caché/frontera LOD y presentación en navegadores con WebGPU operativo.
