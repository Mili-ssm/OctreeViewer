# Temporales del conversor: spools comprimidos y escritura por subárbol

Estado: en curso · 20-11-2026 · fase B hecha ([plan](temporales-del-conversor-fase-b.md), [registro](../records/benchmarks/TEMPORALES_CLI_FASE_B_20-11-2026.md): Albuera 11,1 × → 3,5 × el LAZ, salida idéntica, +24 % de tiempo); falta la fase A

**Objetivo:** bajar el disco temporal del CLI de **11,1 × el LAZ** (medido en Albuera) a **≤ 2 ×**
([RQ-06](../requirements.md#requisitos-de-calidad), [OQ-04](../requirements.md#preguntas-abiertas), #24) sin cambiar
un solo byte de la salida, para que convertir nubes de cientos de GB (ESC-1) no exija varios TB libres.

**Decisión del propietario (19-11-2026):** se hacen A (escritura por subárbol de la raíz) y B (spools comprimidos);
C (releer el LAZ por octante) queda aparcado en #106, a revisar solo si se integra sin perder mucho tiempo.

## Medida de partida

Albuera (`SensOFOREST_2026_02_22_Albuera…laz`, 508 809 556 puntos, LAZ 4 305 484 789 B = 8,46 B/punto), `main` en
`dc98e5a`, perfil `release`, opciones por defecto (v7, 4 workers espaciales), muestreo de 1 s del directorio
`.work` (equipo del propietario, /home en btrfs):

| Dato | Valor |
| --- | --- |
| Pico de temporales | 47 953 682 934 B a los 430 s = 94,2 B/punto = **11,1 × LAZ** |
| Salida (`.oct` + `.octattrs`) | 4 563 202 265 + 11 188 713 371 B = 3,7 × LAZ (fuera de RQ-06) |
| Tiempo / RSS máx. | 9 min 22 s / 243 MB; `verify` correcto |
| Registro de spool de Albuera | 42 B (inferido: la transposición por planos solo comprime con paso 42) |

Compresión de una muestra de 128 MiB de un spool hijo vivo de esa conversión (un solo hilo):

| Códec | Sin planos | Planos de bytes por registro |
| --- | --- | --- |
| `lz4_flex` (ya en el workspace), tramas de 780 / 8192 / 65 536 registros | 1,39–1,43 × | **3,26 / 3,52 / 2,90 ×** (~800–900 MB/s) |
| zstd nivel 1, mismas tramas | 1,86 × | 3,42 / 3,84 / 4,27 × |
| zstd + delta de coordenadas | — | no mejora (3,32–4,19 ×) |

«Planos de bytes» = dentro de cada trama, el byte *k* de todos los registros va seguido (transposición registro→plano),
como en `viewer_format/examples/byte_plane_bench.rs`. Los spools LOD (registro de 54 B, coordenadas f64; muestra de
dos spools LOD de Belvedere) comprimen más: **4,94 / 5,13 / 5,16 ×** con `lz4_flex` y planos (~1 GB/s).

## Por qué el pico es de ~2 copias hoy

Desglose medido (Albuera, instantáneas de 2 s de cada fichero de `.work`; Belvedere al empezar la escritura), con el
tipo de spool deducido del paso de registro (42 B hoja o hijo, 54 B LOD):

| Momento | Hojas (42 B) | LOD (54 B) | Total |
| --- | --- | --- | --- |
| Albuera, pico (420 s, fin de la planificación) | ~0,9–1,0 copias | **~0,84–1,0 × N registros** | 47,97 GB, 25 823 ficheros |
| Belvedere, inicio de la escritura | ~0,87–1,0 copias | ~0,5–0,6 × N registros | 6,05 GB (74 B/punto) |

1. La partición de la raíz copia **todos** los puntos a los spools de sus hijos: 1 copia sin comprimir. Cada nodo
   que se divide borra su entrada al terminar (`node_job.rs:465`, `builder.rs:546-548`), así que las hojas suman
   1 copia que solo se borra al escribir sus bloques (`node_job.rs:1116-1144`, `block.rs:39-44`).
2. **La segunda copia son los LOD**: cada nodo interno guarda hasta ~65 000 muestras (rejilla 256 en v7) con
   coordenadas f64, y ninguno se borra hasta la fase de bloques, que empieza con el plan entero
   (`las_source.rs:384-447`). En Albuera suman casi tantos registros como puntos tiene la nube.
3. El paralelismo no influye: con `--spatial-workers 1` el pico es el mismo (47,98 GB).

## Requisitos

| Id | Requisito |
| --- | --- |
| T1 | Con la misma entrada y opciones, `.oct` y `.octattrs` son **byte a byte iguales** a los de `dc98e5a` (SHA de Belvedere y Albuera) y la paridad web/CLI de `scripts/check.sh` sigue en verde ([RQ-08](../requirements.md#requisitos-de-calidad)). |
| T2 | En Albuera con opciones por defecto, el pico de bytes físicos del directorio `.work` es **≤ 2 × LAZ (≤ 8 610 969 578 B)**. Se mide igual que la línea base (muestreo de 1 s, tamaño **aparente**, lo que ocuparía en ext4 o NTFS) y con el contador del propio conversor (T5). En el equipo del propietario, `/home` es btrfs con `compress=zstd:1`: el uso real se informa aparte (delta de `df`), y la línea base real de Albuera fue ~20,4 GB frente a 47,9 GB aparentes. |
| T3 | El tiempo total de Albuera se mide tras B y tras A frente a 9 min 22 s. No hay tope fijo: el propietario acepta perder más tiempo si la compresión compensa, porque se puede recuperar después con paralelismo (19-11-2026). Una pérdida > +25 % se le presenta con la ganancia de disco antes de seguir. RQ-07 sigue cumpliéndose. |
| T4 | El RSS pico sigue dentro del presupuesto estimado del conversor (512 MiB por defecto); los búferes nuevos están acotados y contabilizados. |
| T5 | El informe del conversor (`peak_live_spool_bytes`, `plan.rs:229-234`) **añade** los bytes físicos en disco (escritos y pico vivo) junto a los lógicos, que los tests comparan con el almacén en memoria (`lib_tests.rs:1503-1516`, `:2150-2161`); los eventos por nodo siguen siendo lógicos. |
| T6 | Ante un error o una cancelación no se publica salida y el directorio `.work` se borra, como hoy. El tipo de error para cada caso no cambia, aunque pueda aparecer después de escribir bloques en el temporal de salida. |
| T7 | El conversor web sigue produciendo los mismos bytes y sus tests siguen en verde; su almacén de spools (RAM + OPFS) no cambia en este plan. |

## Enfoques

| | Enfoque | Pico estimado | Coste | Decisión |
| --- | --- | --- | --- | --- |
| A | Escribir los bloques de cada nodo en cuanto se confirma, un hijo de la raíz tras otro; los LOD ya no se acumulan | ~1,1–1,4 copias (5 × LAZ sin B) | Cambia el orden del pipeline y la API del escritor; menos paralelismo arriba del árbol | Elegido |
| B | Comprimir los spools (planos de bytes + LZ4 por trama) | ÷ 3,3–3,5 (hojas), ÷ ~5 (LOD) | CPU; cambio local en el almacén nativo de spools | Elegido |
| C | Releer el LAZ por octante en vez de copiar la raíz | ~1/8 de copia (1–2 × LAZ) | ~8 × la descompresión del LAZ | Aparcado (#106) |

A sola no basta (la copia de la partición de la raíz es un suelo de ~5 × LAZ) y B sola tampoco (~2,7–3 × LAZ:
24 GB de hojas ÷ 3,4 + 24 GB de LOD ÷ 5, estimado);
juntas dan **~1,6–2,0 × LAZ** (estimado: 1,1–1,4 copias × 42 B ÷ 3,4 ÷ 8,46 B).
zstd comprime algo más que LZ4, pero el crate `zstd` envuelve C y la alternativa en Rust puro comprime poco
(sin verificar); se reconsidera solo si B+A no cumplen T2.

## Diseño

**Orden:** B primero (cambio local, de riesgo bajo, medible por sí solo), después A. Se mide T2 tras cada uno.

### B: spools comprimidos en el almacén nativo

- Solo cambia `FileSpoolStore` (`crates/viewer_convert/src/file_spool_store.rs`); el trait `SpoolStore`, el almacén
  web (`core/pipeline/hybrid/`) y el de tests no cambian, salvo lo mínimo para que el almacén conozca el tamaño de
  registro de cada spool (o por `SpoolId` + la `SpoolLayout` al crearlo, o con un parámetro en `create`: decisión
  del constructor).
- Cada spool abierto acumula en RAM una trama de hasta 8192 registros completos; al llenarse o en `close`, transpone a
  planos de bytes, comprime con `lz4_flex` y la añade al fichero.
- El almacén guarda por spool una tabla de tramas en RAM (offset lógico → offset y longitud físicos). `len()` sigue
  siendo lógico. `reader(extent)` busca la trama del offset inicial, descomprime y salta lo que sobra: las lecturas
  por offset de `PlannedBlock::read` (`block.rs:51-61`) y las lecturas paralelas del mismo spool siguen funcionando.
- `join_segments` (`node_job.rs:823-847`) puede descomprimir y recomprimir; copiar tramas sin descomprimir es una
  mejora posterior.
- El `source.spool` de `--checkpoint`/`--resume` sigue en crudo (los formatos paginados, como v7, ya rechazan el
  checkpoint); así no cambia `MANIFEST_VERSION`.

### A: escritura por subárbol de la raíz, en el mismo orden que hoy

- Los ids de bloque ya se asignan en preorden al confirmar cada nodo (`builder.rs:528-555`, `:636-665`) y los
  payloads se escriben en ese orden (`writer.rs:331-371`). A escribe cada bloque cuando su nodo se confirma, en ese
  mismo preorden, así que los bytes no cambian (T1).
- **Escritor** (`viewer_format/src/writer.rs`): un modo que recibe al principio lo que el encabezado y la
  codificación de bloques necesitan (versión, esquema, marco global y retícula de la raíz) y la jerarquía completa al
  final, en `finish_pair`, donde se valida y se construye el índice como hoy. Hoy `new_with_layout` exige los
  metadatos completos y rechaza bloques fuera de la jerarquía (`writer.rs:241-275`, `:375-386`); la caja de cada
  bloque sale de `bind_blocks` sobre todos los nodos (`prepare.rs:80-84`), y ha de poder calcularse por nodo
  (`global_frame.rs:496-518`).
- **Planificador** (`spatial/builder.rs`): al confirmar un nodo, entrega sus `PlannedBlock` (con id global) a un
  sumidero; el del CLI los prepara y escribe en orden con el pipeline existente (`las_source.rs` `process_batches`)
  y borra cada spool tras su último bloque, como hoy.
- **Paralelismo** (`viewer_convert/src/executor.rs`): un hijo de la raíz por lote, con W hilos en la cola de trabajos
  listos (hoy el número de hilos es el de hijos del lote). Los hilos se mantienen ocupados en cuanto el hijo de la
  raíz se divide; se pierde el paralelismo de la primera división de cada hijo de la raíz (riesgo para T3).
- **Comprobaciones globales** (`plan.rs:236-345`: estimación de memoria, `check_reader_attribute_ranges`,
  `SpatialHierarchy::new`, `TooManyBlocks`) pueden saltar con bloques ya escritos en el temporal de salida; el error
  y la limpieza se mantienen (T6).
- **Web:** el núcleo es compartido (`serial.rs:76-102`). El camino web puede seguir con «planificar y luego escribir»
  (un sumidero que acumula); adoptar A en la web es trabajo aparte.

### Puntos ya resueltos (revisión del diseño, 19-11-2026)

- **Tamaño de registro:** varía con los atributos de la fuente y es mayor en los spools LOD (coordenadas f64,
  `spool.rs:60-78`); la transposición es una sola función con paso de registro en tiempo de ejecución, tomado de la
  `SpoolLayout`. Los 42 B de Albuera son solo la medida de esa nube.
- **Orden de escritura:** `run_and_commit` (`builder.rs:528-555`) confirma el nodo tras su trabajo y antes de sus hijos,
  y el ejecutor con hilos confirma con un cursor en preorden (`executor.rs:1-12`); el LOD de un nodo interno sale de su
  propia entrada, no de sus hijos (`node_job.rs:368-457`). Escribir en el momento de la confirmación conserva el
  orden actual de payloads; no hace falta búfer de reordenación.
- **Encabezado y marco:** el encabezado solo depende de la versión y el papel del fichero (`writer.rs:263`,
  `:295-304`); el prefijo global de cada nodo, de sus límites y de la base de la raíz (`plan.rs:355-363`), conocida
  antes de planificar. El índice va tras los payloads y se escribe en `finish_pair`: la salida se sigue escribiendo
  solo añadiendo al final, sin reescribir nada.
- **Errores:** el temporal de salida se borra al soltarse (`publish.rs:352-364`) y `.work` también; los formatos
  paginados (v7) rechazan `--checkpoint`, y con checkpoint (formatos heredados) se mantiene el flujo actual.
- **Referencias de T1:** la primera tarea genera los SHA de Belvedere y Albuera en `dc98e5a` y los guarda en el
  registro de medidas; los datasets están en `data/datasets/` del checkout principal.
- **T2 es objetivo, no puerta dura** (RQ-06 es técnico y blando en el
  [catálogo](../requirements.md#catálogo-funcional-o-técnico-duro-o-blando-aceptado-19-11-2026)): si A+B no llegan,
  se registra la medida y se lleva al propietario la elección entre zstd, C (#106) o aceptar la cifra, sin más
  trabajo antes. Tras B se mide también Majadas, como dato y no como puerta.

## Fuera de alcance

- El tamaño de la salida (3,7 × LAZ en Albuera, sobre todo `.octattrs`): RQ-06 solo cubre temporales.
- El conversor web y su estimación de cuota OPFS (`hybrid/estimate.rs` asume registros sin comprimir).
- Formatos que no sean los que hoy convierte el CLI desde LAS/LAZ; EPT no cambia.
- C (#106) y el límite de ramas paralelas por espacio libre en disco.

## Riesgos

| Riesgo | Mitigación |
| --- | --- |
| La compresión de otras nubes (Majadas, TLS) o de los LOD de Albuera sea peor que la medida (3,3 × hojas de Albuera, 5 × LOD de Belvedere) | Medir en B con Albuera y Majadas antes de empezar A |
| A pierda tiempo por la primera división secuencial de cada hijo de la raíz | Medir tiempo total y por fase en B y en A; si supera P1, permitir adelantar el siguiente hijo cuando haya hilos ociosos |
| Tests de orden de errores, progreso por fases y paridad dependan del orden actual | Mantenerlos en verde; cambiar un test solo si su expectativa era el orden interno, nunca la salida |
| La CPU de comprimir alargue la partición de la raíz | LZ4 ~800–900 MB/s por hilo frente a ~5 GB de spools por minuto de hoy (estimado); medirlo |

## Verificación

1. `scripts/check.sh --offline` en verde (incluye paridad de bloques reales con Belvedere y SHA del CLI).
2. SHA de `.oct`/`.octattrs` de Belvedere y Albuera iguales a los de `dc98e5a` (T1).
3. Pico de `.work` en Albuera con muestreo de 1 s y con el contador físico (T2, T5), tras B y tras A; tiempo total y
   RSS con `/usr/bin/time -v` (T3, T4); registro fechado en `docs/records/benchmarks/`.
4. Tests nuevos: ida y vuelta de tramas (lectura por offset dentro de una trama, entre tramas y al final), error y
   cancelación a mitad de escritura (T6).

## Preguntas resueltas

| Id | Pregunta | Respuesta del propietario (19-11-2026) |
| --- | --- | --- |
| P1 | ¿Cuánto tiempo extra de conversión se acepta a cambio de T2? | «Depende: si la compresión es suficientemente buena puedo permitir perder más tiempo, pues luego lo podríamos recuperar con paralelismo» (recogido en T3). |
