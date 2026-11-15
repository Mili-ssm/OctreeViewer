# V7: almacenamiento y ejecución acotada

Estado: activo · 30-10-2026

Decisión del 24-10-2026: dedicar el desarrollo al sucesor V7 y retirar V5/V6 al completar
su sustitución. El [estado](../estado.md#prioridades-pendientes) registra el progreso activo.
Este documento fija el contrato, las alternativas y los criterios de cierre. La base V7.1-pre
y el plan selectivo de lectura están implementados; las integraciones pendientes se indican
en cada fase y en el README.

## Resultado y límites

Reducir el tamaño de la pareja persistida y los picos de memoria y disco temporal, manteniendo
lecturas selectivas e integrando concurrencia en reader y writer. Conservar filas, duplicados,
asociación XYZ/RGB/atributos, bits de atributos tipados (incluidos NaN y cero con signo), error
de cuantización declarado, marco local, jerarquía y semántica Replacement/Additive.
No mezclar este cambio con una reescritura del renderer ni con nuevos contratos de CRS.

La asincronía solapa esperas; el paralelismo distribuye CPU y puede multiplicar buffers.
La velocidad se medirá junto a los recursos consumidos. No se presupone que permita compensar
cualquier codec más costoso. El objetivo de más de 500 GB sigue sin demostrarse.

Objetivos acordados (30-10-2026) en [requisitos](../requirements.md): equipo EQ-REF, PSS del visor RQ-05,
disco temporal RQ-06 y tolerancias de V7 frente a V5 RQ-07; alcance y congelación en el
[ADR 0022](../adr/0022-alcance-y-congelacion-de-v7-final.md). RQ-07 (cifra de conversión revisada por el
[ADR 0043](../adr/0043-coste-de-conversion-de-v7-frente-a-v5.md)) y RQ-06
(temporales ≤ 2 × LAZ) son los objetivos vigentes; las mediciones actuales de §4 y §6 aún no los demuestran.

**Alcance de V7 final:** pirámide densa (R4, [ADR 0020](../adr/0020-piramide-lod-densa.md)), jerarquía
paginada (R5), rejilla global determinista y CRS ([ADR 0021](../adr/0021-rejilla-global-determinista-y-crs.md)),
flags LAS y Extra Bytes conservados, escritor que rechaza > 256 atributos ([ADR 0007](../adr/0007-limites-por-defecto-del-lector.md)) y codificador LZ4
propio ([ADR 0025](../adr/0025-codificador-lz4-propio.md)). Congelado el 15-11-2026 con RQ-07 revisado ([ADR 0044](../adr/0044-v7-final-congelada-y-predeterminada.md)): V7 es la salida por defecto; el codificador LZ4 propio sigue pendiente.

## Base contrastada

| Base inspeccionada | Conservar / corregir |
| --- | --- |
| [V5](../reference/format/FORMAT_V5.md): separación `.oct`/`.octattrs`, columnas y CRC independientes. | Abrir XYZ sin acompañante; RGB o una columna sin leer las demás; no compartir IDs entre columnas por defecto. |
| [V6 pre4](../records/archive/FORMAT_V6_PRE4.md): delta, ocupación compacta, constantes, diccionarios y planos de bytes con fallback. | Reutilizar codecs y validaciones; contrastar tamaño completo y coste de candidatos. La ocupación por bloque no define una grilla global. |
| [I/O existente](../records/benchmarks/STORAGE_V6_IO.md): preparación Rayon, pipeline optativo con hasta dos lotes, `fork_reader` e índice compartido. | Reutilizar límites de propiedad y escritura ordenada; límites por lote y modelos actuales no garantizan RSS. La lectura paralela del ejemplo no está integrada en el viewer. |
| [`BlockRequestScheduler`](../../crates/viewer_core/src/scheduler.rs) y [reader](../../crates/viewer_format/src/contracts.rs). | Prioridad, tickets y cancelación lógica existentes; integrados en WASM con admisión por bytes antes del ticket. El presupuesto del viewer/cache y la medición conjunta siguen pendientes. |
| [Memoria de conversión](../records/benchmarks/V5_MEMORY_BENCH.md) y [memoria web](../records/benchmarks/WEB_PERFORMANCE_BENCH.md). | Separar spools, buffers, índice, caché, WASM y GPU; las medidas existentes pertenecen a sus versiones y entornos. |

## 1. Base V7 verificable

Primera revisión optativa [`v7.1-pre`](../reference/format/FORMAT_V7_PRE1.md), con identidad
propia y reglas de payload heredadas de pre4. La alternativa de diseñar todo el formato final
antes de disponer de una ruta ejecutable retrasaría las comprobaciones; la base provisional
permite validar conversión, selectividad y checkpoints mientras cambia la representación.
Su nueva identidad por sí sola no ahorra disco ni RAM. Cada revisión futura será exacta;
V7 final permanecerá rechazado hasta disponer de su implementación y verificación.

Primer cambio de memoria compatible con pre3/pre4 y V7: para columnas de un byte, construir
la paleta de hasta 256 valores y empaquetar desde la entrada sin el `Vec<u32>` de N IDs.
Se elimina una reserva de `4*N` bytes por encoder activo en esa ruta; no es una reducción
medida del pico RSS. Para tuplas más anchas, liberar el lookup antes de empaquetar; liberar
tabla e IDs empaquetados sin comprimir tras usarlos. Conservar candidatos, desempates,
orden de primera aparición y payloads exactos. La prueba diferencial cubre la compatibilidad.

Salida del incremento: reader/writer versionados, CLI, conversión LAS/LAZ y EPT local,
pipeline y checkpoints con identidad exacta; V5 era el default operativo durante
la transición (V7 lo es desde el 15-11-2026, ADR 0044). La conversión web y sus defaults no cambian en este incremento.

## 2. Presupuestos y medición antes de ampliar concurrencia

**Implementado:** `FormatReader::plan_read(id, ReadSelection)` obtiene desde el índice hasta
dos rangos para XYZ, XYZ+RGB, RGB o una columna. Cada rango identifica archivo principal o
atributos, offset absoluto, tamaños stored/raw, codec y CRC. El plan no accede a payloads ni
construye el catálogo completo. Valida bloque, columna, acompañante, tamaños y aritmética.
XYZ+RGB sin acompañante conserva la lectura gris de XYZ; pedir RGB o una columna sin él falla.

`read_selected(id, selection, max_peak_bytes)` recalcula el plan y rechaza con
`ReadBudgetExceeded` antes del I/O si supera la estimación permitida. Devuelve `SelectedRead`
tipado. Recalcular impide ejecutar un plan suministrado por el llamante contra otro dataset.
Los métodos síncronos anteriores conservan sus contratos; no adquieren un presupuesto implícito.

Para un payload con S bytes almacenados, R raw y O de salida tipada, el modelo estima el
máximo entre descompresión y materialización. No necesita leer la cabecera interna del codec:

| Etapa de descompresión | Bytes estimados |
| --- | --- |
| Raw | S; el buffer almacenado se reutiliza. |
| LZ4 / constante | S + R. |
| Planos de bytes | S + 2R: planos y salida transpuesta. |
| Diccionario | S + 3R: tabla, IDs empaquetados y salida; tabla e IDs están acotados cada uno por R. Es una cota conservadora sin inspeccionar payload. |
| Materialización normal | R + O. |
| Materialización XYZ con ocupación | R + O + 12N + ceil(N/8), contando coordenadas y validación de filas. Algunas vidas no coinciden: es conservador. |

Para XYZ+RGB: `max(pico_xyz, salida_xyz + pico_rgb)`, porque se conservan los puntos mientras
se lee RGB y se actualizan en el mismo vector. `stored_bytes` y `decoded_bytes` suman tráfico de
los rangos; `output_bytes` es la salida final retenida. Esos campos no se suman para obtener
`peak_bytes`. Los tamaños usan el `size_of` del target para puntos/RGB y el esquema para atributos.

El presupuesto de `read_selected` limita **la estimación por lectura**, no RSS. La cuota
compartida descrita a continuación contabiliza tareas concurrentes y salidas retenidas.
El modelo excluye índice, caché de transporte, strings/metadatos de esquema, pequeñas estructuras
de control/DFS, runtime y sobrecoste/capacidad extra del asignador. Aún falta medir los picos
reales y completar el diagnóstico por fases. El benchmark `real_dataset_bench`
usa esta API en XYZ/columna, compara bytes planeados con los realmente leídos e informa
`max_estimated_read_payload_peak_bytes`. Sus tiempos selectivos incluyen ahora planificación
y preflight, por lo que no deben tratarse como un A/B del decoder anterior.

**Cuota compartida implementada:** `ReadBudget` usa reservas RAII no clonables y movibles entre
threads. `try_reserve` distingue petición mayor que el límite (error) y capacidad ocupada
(`None`); no bloquea el hilo esperando una cuota que puede retener el propio llamante. Reservar
cero bytes es válido, incluso con límite cero. Los contadores usan aritmética comprobada.

`try_read_selected(id, selection, &budget)` reserva el pico antes del I/O. `read_reserved`
consume un permiso adquirido antes de despachar el trabajo, valida su capacidad contra el
plan del lector efectivo y lo libera si falla. Al terminar los buffers temporales, `BudgetedRead`
reduce la reserva a `output_bytes`, conserva resultado y permiso juntos y destruye la salida
antes de liberar la cuota. Expone solo préstamo inmutable: no permite extraer el buffer y
perder su reserva. Copias explícitas del consumidor quedan fuera de esta contabilidad.

Separar el presupuesto de índice/caché del de trabajo en vuelo evita doble conteo. Abandonar
una petición o cambiar de objetivo no libera el permiso de un worker que aún tiene buffers:
se libera al terminar y descartar. El límite se aplica al modelo lógico, no al RSS. Los contadores
`reserved_bytes` y `peak_reserved_bytes` son de toda la vida del presupuesto, incluidos sus clones.

**Pendiente de medición conjunta:**

Ampliar diagnósticos existentes por fases, sin muestrear el árbol de temporales por cada punto:
entrada/planificación, preparación, escritura, cierre/sync, validación y publicación. Registrar
bytes lógicos vivos y asignados en disco por separado cuando el sistema lo permita, RSS/PSS
muestreados y su frecuencia. No sumar picos de fases distintas como si fueran simultáneos.

## 3. Reader lógico único, concurrencia y transporte asíncrono

**Criterio de biblioteca:** `AsyncFormatReader` es clonable para que varias tareas puedan invocar
`read` concurrentemente sobre el mismo estado lógico. Una instancia y sus clones comparten índice
validado, handles de la misma pareja de archivos, límite de operaciones en vuelo y cuota de memoria.
Internamente crea un cursor privado por tarea; callers no administran arrays de readers ni cursores.

**Criterio de aplicación:** durante una sesión/dataset, el viewer conserva una única instancia
propietaria del reader y distribuye referencias/clones livianos a las tareas que la necesiten.
Abrir otro dataset reemplaza ese servicio en el límite de ciclo de vida, coordinando generaciones
y resultados en vuelo. La clonabilidad es una propiedad de la API de biblioteca; no prescribe
crear varios readers para un mismo dataset en la aplicación.

La primera interfaz es nativa y optativa con `viewer_format/async-reader` y Tokio. `open` valida
una vez el par de archivos; cada `read(id, selection)` calcula su plan sin I/O de payload, espera
un slot y cuota suficientes, y ejecuta I/O bloqueante más decodificación en el pool bloqueante de
Tokio. En Unix las lecturas posicionadas comparten el handle sin compartir posición. En otras
plataformas el fallback protege `seek/read` con un mutex por archivo; la decodificación queda
fuera del mutex. La API no afirma que el disco use I/O nativo no bloqueante.

El consumidor elige `max_in_flight` y `max_payload_bytes` al abrir. No se reserva un slot mientras
una llamada espera cuota, por lo que una petición pequeña puede avanzar aunque otra grande no
quepa en el espacio restante. `ReadBudget` cubre picos lógicos planeados durante el trabajo y
retiene la reserva de salida mientras exista algún `Arc<BudgetedRead>`. Soltar o cancelar el
future después de iniciar `spawn_blocking` no cancela el worker: este conserva slot y reserva
hasta acabar y destruir/entregar su resultado, de acuerdo con el [contrato de Tokio para
`spawn_blocking`](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html). Es necesario
para no admitir trabajo que exceda la cuota real contabilizada; el cierre del dataset tampoco
invalida un handle en uso.

Ejemplo de uso conceptual:

```rust,ignore
let reader = AsyncFormatReader::open("data.oct", Some("data.octattrs".into()), 4, budget_bytes).await?;
let xyz = reader.read(block_id, ReadSelection::Positions).await?;
let rgb = reader.clone().read(block_id, ReadSelection::DisplayColors).await?;
```

El `Arc<BudgetedRead>` mantiene contabilizado el payload hasta que se suelten todos sus clones.
Clonar sus datos a buffers propios queda fuera de la cuota. No se incorporan todavía caché,
deduplicación/coalescing ni prefetch: la instancia compartida y los planes por rangos preparan
ese punto de extensión sin fijar una política de caché antes de medirla.

**Implementado en nativo:** `read_selected_parallel` recibe lectores independientes creados
con `fork_reader`, un slice de peticiones, una cuota y un callback prestado. Verifica que todos
comparten el índice validado antes del I/O; los streams de cada fork deben seguir apuntando a
la misma instantánea inmutable, como exige `fork_reader`.

Planifica y reserva solo una ventana de hasta tantos trabajos como lectores. Si la cuota solo
admite uno, lo ejecuta en el hilo llamante; con varios usa threads scoped, reúne todos y entrega
en orden de petición. Así los resultados adelantados quedan acotados. No introduce un pool
persistente ni un runtime; la creación de threads por ventana tiene un coste pendiente de medir.
El lector nativo continúa siendo bloqueante: paralelismo de I/O/CPU no equivale a transporte async.

El callback recibe `&SelectedRead` después de reunir la ventana y mantiene la reserva durante
su ejecución. `ControlFlow::Break` detiene nuevas entregas/admisiones; el progreso cancelado
incluye ese resultado. Los trabajos ya admitidos pueden haber terminado sin entregarse. `Blocked`
no espera: informa el prefijo entregado si una reserva externa impide admitir la siguiente
petición. Reanudar con `requests[delivered..]`; los índices del callback son relativos a esa llamada.

Errores de lectura/CRC o panic de worker identifican petición y prefijo entregado. Se reúnen
los workers y se descartan resultados restantes antes de salir. Un error de preflight puede
rechazar toda la ventana aún no lanzada; `delivered` sigue siendo el prefijo ya entregado.
El panic del callback propaga después de reunir los threads y libera resultados por unwinding;
`panic=abort` no garantiza limpieza. No se interrumpe un codec ni se libera antes su cuota.
Para retener una salida fuera del callback, usar la primitiva individual y conservar `BudgetedRead`.

La herramienta nativa `selective_parallel_bench` permite comparar XYZ, XYZ+RGB, RGB o una columna
con 1/2/4 lectores, checksum lógico y pico de reservas. Su catálogo/peticiones y contadores son
sobrecoste del benchmark y no forman parte de la cuota de payloads.

El ejecutor anterior se conserva como primitiva síncrona de bajo nivel y como instrumento de
comparación; no es el modelo de uso elegido para integrar el reader con el viewer.

**Integración async/visor:**

**Decisión confirmada por el usuario (25-10-2026):** la ruta WASM debe obtener apertura e índice
y payloads mediante `Blob.slice(...).arrayBuffer()` u otra API Blob realmente asíncrona. No se
considera cumplimiento envolver `FileReaderSync` en una tarea o worker.

**Primer incremento implementado:** `FormatReader::materialize_selected_from_buffers` consume
un `ReadPlan` de la misma instancia lógica y los buffers stored de sus rangos. Valida identidad,
cantidad, longitudes y CRC antes de reutilizar los decodificadores de codec, XYZ, RGB y atributos;
no hace I/O de archivo ni cambia los bytes V7. Las pruebas comparan la lectura síncrona para
posiciones, geometría con/sin atributos, RGB y una columna, y rechazan plan ajeno, buffer truncado
y checksum incorrecto.

**Segundo incremento implementado:** `split_index_range` valida cabecera/footer y devuelve el
rango exacto del índice sujeto a `FormatReaderLimits`. `from_split_buffers` valida CRC y construye
el índice residente desde cabecera, footer e índice ya obtenidos; `attach_attribute_identity`
valida el companion desde su cabecera/footer sin exigir tener su payload en un cursor. El reader
marcador rechaza lecturas síncronas de payload; las pruebas V7 comparan resultados con/sin attrs y
rechazan corrupción y companion incompatible. Esto permite consumir rangos obtenidos externamente,
pero todavía no llama `Blob.arrayBuffer()`, admite tareas por bytes ni se conecta al ciclo LOD.

**Tercer incremento implementado:** `WasmAsyncFormatReader` abre Blobs split leyendo cabecera,
footer e índice con `Blob.slice(...).arrayBuffer()`, luego crea planes internos para posiciones,
geometría XYZ+RGB, RGB o una columna tipada y obtiene únicamente sus rangos. El plan queda ligado
al reader y no puede sustituirse desde JS antes de consumirlo; el materializador común valida
longitudes y CRC. Las posiciones/RGB/columnas llegan en typed arrays que conservan el tipo de cada
atributo. Un fixture V7.1-pre reproducible comprueba esta ruta en el navegador, incluidos rangos
sin lectura completa del Blob, colores, columna U16 y rechazo de payload corrupto.

En ese incremento la fachada era un paso de biblioteca WASM separado del viewer. El quinto
incremento la conecta al worker y al ciclo LOD; el sexto añade lecturas simultáneas con presupuesto
compartido; el séptimo integra los tickets del scheduler. Sigue pendiente medir navegación completa.

**Contrato de integración WASM, contrastado con el flujo actual:** `LodDataset::update_view_progressive`
selecciona el corte, invoca el loader para el bloque pendiente de mayor prioridad y vuelve a
preparar el plan. El loader puede responder `None`; así el bloque queda pendiente y la publicación
conserva el corte anterior cubierto. La llamada completa es síncrona hoy y `bindings.rs` mantiene
préstamos mutables de dataset/reader durante el loader, por lo que no se insertará un `await` en
ese callback.

La implementación se separa en límites cortos de estado:

1. **Abrir:** el worker obtiene con `Blob.slice(...).arrayBuffer()` cabecera, footer, índice acotado
   y cabecera/footer del companion. WASM construye el reader metadata-only con los límites de
   `FormatReaderLimits`; nunca solicita el archivo completo.
2. **Planificar:** el controlador conserva selección LOD, prioridad y preparación del corte. Para
   un reader externo, registra el bloque pedido y devuelve su `ReadPlan` con identidad de dataset,
   generación del objetivo, selección, rangos y coste estimado. Luego libera todos los préstamos.
3. **Leer:** el worker reserva primero operaciones y bytes, y obtiene los rangos exactos con Blob
   asíncrono fuera de la cola de mutaciones del viewer. La admisión cubre `ArrayBuffer` JS, copia a
   WASM, scratch y resultados que aún no llegaron a caché.
4. **Aceptar/publicar:** una transición síncrona valida ticket, dataset y generación; materializa
   CRC/codec con el plan retenido en WASM, inserta el bloque y prepara/publica el corte. Una
   respuesta obsoleta o fallida se descarta sin retirar el corte visible ni liberar cuota antes de
   soltar sus buffers.

El séptimo incremento conecta `BlockRequestScheduler` en esos límites, con prioridades derivadas
del orden del plan LOD existente y sin un segundo selector. Limita slots y bytes simultáneamente:
la cuota Blob se reserva antes de tomar un ticket, y las cancelaciones lógicas mantienen su slot
hasta completar el ticket. La apertura también contabiliza el índice residente. El renderer
consume escenas; no abre archivos. El worker procesa mensajes de cámara/cierre mientras Blob
resuelve y serializa únicamente las transiciones WASM breves; no mantiene préstamos WASM durante
`await`.

Como paso de caché posterior, medir primero deduplicación de peticiones exactas por identidad de
dataset, bloque y selección. Después comparar coalescing de rangos solapados/adyacentes cuando
reduzca I/O real sin ampliar demasiado buffers, latencia de cola o retención de resultados. La
presión de memoria debe poder expulsar resultados sin invalidar `Arc` ya entregados; prioridad y
fairness del scheduler se contrastan con ese coste. Ninguna de estas políticas está implementada
por el reader async actual.

Consumir resultados por identidad al estar listos y descartar generaciones obsoletas. Preservar
el corte/fallback válido ante fallos parciales; reintentos explícitos y cancelación al cerrar
dataset. El benchmark mantiene orden/checksum determinista con ventana acotada: un bloque lento
no puede hacer crecer una cola de reordenación ilimitada. Coalescing/prefetch solo con límites
de bytes extra y medidas de solicitudes, seeks y latencia del almacenamiento objetivo.

## 4. Writer y pico de disco

**Avance acotado — EPT LASzip:** `point_data_to_block` ahora recorre dos veces el iterator RGB
reutilizable de `PointData` para construir el color de display y la columna RGB tipada, sin
materializar antes un `Vec<[u16; 3]>` que luego se aplanaba a otro `Vec<u16>`. El cambio elimina
una representación intermedia adicional de 6 bytes por punto coloreado durante la conversión del
bloque; conserva los canales U16 exactos y el color de display. El fixture V7 nuevo comprueba ambos.
Es un coste de memoria teórico deducido de los buffers retirados, no una medida de RSS; el tamaño
en disco y la concurrencia de EPT no cambian.

Reutilizar preparación paralela + único escritor ordenado. Sustituir la admisión basada solo
en cantidad de bloques por costes conservadores de entrada, candidatos, payload preparado y
ambos lotes del pipeline. Un bloque que no cabe debe provocar rechazo explícito o subdivisión
durante planificación; no superar cuota silenciosamente. La salida debe ser determinista con
1/2/4 workers.

**Implementado para EPT:** `--pipeline-writes` recorre cada LAZ y sus bloques en orden, prepara
un bloque por vez en un productor y lo entrega por canal de capacidad cero al escritor del hilo
llamante. El canal no forma una cola; como máximo se solapan el bloque que se escribe y el
siguiente que se lee/prepara. Se activa solo si el presupuesto estimado deja margen de 192 bytes
por punto del bloque mayor; si no, usa el recorrido serial. No añade workers de lectura/codec ni
checkpoint EPT. Una prueba de fixture compara byte a byte `.oct` y `.octattrs` entre ambas rutas,
también con un presupuesto que fuerza el fallback. Esto valida orden y equivalencia en el fixture,
no un speedup ni el RSS real.

`convert_file_with_diagnostics` recoge para EPT ventanas de planificación (manifest, jerarquía y
topología), lectura/preparación/escritura de bloques y cierre/sync de la pareja. No atribuye bytes
de spools, workers internos, validación ni memoria; esas mediciones siguen siendo externas y deben
quedar explícitas como tales.

**Recuperación EPT V7.1-pre implementada:** el writer aporta registros binarios versionados por
bloque; el convertidor sincroniza ambos candidatos, anexa/sincroniza el diario y reemplaza/sincroniza
el manifest antes de anunciar el progreso. Reanudar valida el prefijo, restaura los registros y
trunca solo colas posteriores a sus offsets. También retoma dentro de un LAZ con `Reader::seek` y
recupera la publicación desde `OutputReady`; los bytes V7 publicados no cambian. El inventario de
fuentes se guarda una vez en `sources.bin`, separado del manifest que se sincroniza por bloque. El
checkpoint liga opciones y versión del plan de conversión. La identidad de fuente compara rutas,
tamaños y marcas de modificación de `ept.json`, páginas de jerarquía alcanzables y LAZ, con
verificaciones antes/después de lectura y antes de publicar. No hashea el contenido: cambios que
conserven esos metadatos pueden pasar inadvertidos y no se obtiene una instantánea inmutable. Las
pruebas simulan interrupciones mediante cancelación entre bloques y antes de publicar; no inyectan
pérdida de energía ni terminación forzada del proceso.

Verificación ejecutada: `cargo test -p viewer_convert --offline` (59 pruebas de biblioteca y 1 de
CLI), `cargo test -p viewer_format --offline --lib` (91 pruebas, 1 ignorada) y
`cargo clippy -p viewer_convert --all-targets --offline -- -D warnings`. No cambia el formato V7
en disco ni certifica disponibilidad de GPU/navegador.

Medir disco simultáneo: spools fuente/hijos/LOD + candidatos `.oct`/`.octattrs` + pareja anterior
durante sustitución + checkpoints conservados. Comparar liberación temprana del último rango
con spools comprimidos/chunks: menos bytes puede exigir más CPU y perjudicar lecturas parciales.
Elegir según medidas de tráfico y pico; no cambiar checkpoints sin versionarlos.

**Siguiente cierre medible:** repetir las conversiones actuales para estimar dispersión y completar
Belvedere/Majadas con 1/2/4 workers; medir V7 con/sin checkpoint y EPT con/sin `--pipeline-writes`.
Registrar RSS/PSS muestreados, bytes temporales máximos, hashes y capacidades retenidas de lotes.
El profiler de filesystem puede perder picos breves o spools eliminados durante el recorrido; sus
muestras no son una cota exacta de disco físico. Usar esa base antes de reemplazar la estimación de
memoria actual por admisión ponderada por bytes, y aceptar el cambio solo si reduce el pico medido
sin cambiar salida, orden ni recuperación.

**Comparación secuencial Albuera V5/V6.4/V7:** una conversión por versión, en ese orden, mismo
host, binario `perf`, fuente (4.305.484.789 B; SHA-256 en el JSON), un worker, divisiones 2 y
262.144 puntos/bloque, sin checkpoint ni cargas de test/build simultáneas. Todas completaron.
V5 tomó 478,01 s y produjo 13.666.843.763 B; V6.4-pre tomó 809,36 s y V7.1-pre 796,84 s,
ambas con 10.320.734.767 B. En esta muestra V7 conserva el tamaño V6.4 y ocupa **3.346.108.996 B
menos (24,48%) que V5**. Los picos RSS muestreados fueron 111.628.288 / 119.267.328 / 123.494.400 B;
los picos temporales 27.460.988.122 / 27.456.721.085 / 27.463.379.529 B. No se observa una
reducción del pico de memoria/temporales con V7. Una sola corrida por versión y caché del sistema
sin purgar no estiman dispersión ni permiten atribuir las diferencias de tiempo al layout.

`storage_bench progressive` leyó cada pareja en el mismo orden y confirmó en las tres 8.437
bloques, 508.989.347 puntos incluyendo LOD y el mismo checksum BFS `8caae2a09753e50e`; cubre IDs,
coordenadas/color reconstruidos, nombres/tipos de columnas y valores tipados. Esto establece
igualdad lógica para esta fuente. Las lecturas tardaron 63,36 / 68,87 / 64,34 s; al ser una lectura
por salida sin control de caché, son observaciones, no evidencia comparativa de latencia. Reportes
compactos, opciones, hashes de binarios, temporales y límites:
[la serie secuencial Albuera](../records/benchmarks/data/storage-v7-albuera-v5-v64-v71-sequential-2026-10-25.json).

**V7 Albuera por workers:** una pasada sin checkpoint ni pipeline, mismo host/binario y orden
1/2/4 workers. El tiempo fue 796,84 / 662,53 / 811,36 s; 2 workers observó 16,86% menos tiempo
que 1, mientras que 4 tardó 1,82% más. RSS máximo muestreado: 123.494.400 / 243.589.120 /
400.592.896 B; temporales: 27.463.379.529 / 29.258.309.075 / 35.911.878.607 B. La capacidad
codificada máxima por lote observada pasó de 5.900.424 a 11.345.638 y 19.631.592 B; los bloques
preparados máximos fueron 1/2/4. Las tres parejas mantuvieron el mismo tamaño y checksum lógico
(`8caae2a09753e50e`, 8.437 bloques, 508.989.347 puntos). Es una observación por configuración,
con caché no purgada; el resultado de 2 workers no justifica cambiar la política general todavía.
[Matriz parcial y diagnósticos](../records/benchmarks/data/storage-v7-albuera-v71-workers-2026-10-25.json).

Error/cancelación: desbloquear canales, reunir workers y después limpiar temporales. No prometer
cancelación dentro de un codec. Mantener fuente/opciones/revisión en checkpoint; nunca reanudar
con otra revisión. Cerrar, sincronizar y validar antes de publicar. Auditar recuperación en cada
punto de publicación: dos renombrados no hacen atómica una pareja. No borrar datasets existentes
para forzar la migración; regenerar y validar reemplazos primero.

## 5. Representación final: candidatos condicionados

| Decisión | Primera alternativa sencilla | Alternativa a medir / contraejemplo |
| --- | --- | --- |
| XYZ | Mantener raw/delta y ocupación compacta exacta por bloque. | Bitpacking por rango o bloques internos más pequeños; incluir duplicados, coordenadas dispersas, orden y metadatos. Una topología menor puede necesitar una permutación costosa. |
| Atributos | Columnas independientes, constantes/diccionario/planos y raw/LZ4 fallback. | Bitpacking por rango y delta por componente con preservación de bits; alta cardinalidad y flotantes ruidosos pueden perder. No reinterpretar NaN como valores equivalentes. |
| Elección de codec | Comparar tamaño final de candidatos uno tras otro, reutilizando buffers. | Filtros de candidatos basados en perfil; el muestreo puede descartar el mejor y exige cuantificar la pérdida. |
| Índice | Índice plano validado compartido, sin duplicarlo por worker. | Directorio compacto o paginado solo si apertura/residencia domina; medir solicitudes extra, validación incremental y acceso aleatorio. |
| Payloads | Una unidad independiente por columna/bloque. | Páginas internas para limitar scratch/lecturas por fila; cuentan tablas, padding, CRC y bytes por solicitud. |
| Spools | Liberar tras último uso con el orden/checkpoint vigente. | Chunks comprimidos o reducir pasadas; comprobar pico total, CPU y reanudación. |

No integrar todos los candidatos: elegir con evidencia. Mantener split inicialmente; un contenedor
único simplifica publicación, pero cambia asociación de acompañantes y acceso a geometría sola.
Reconsiderarlo solo si la recuperación de pareja resulta un problema dominante demostrado.

## 6. Validación, promoción y retirada

**V5/V7 repetidos en Belvedere y Majadas:** mismo binario, un worker, divisiones 2 y bloques de
262.144. Dos pares en Belvedere y tres en Majadas, en órdenes alternados; la caché no se purgó.
Todas las salidas conservaron el checksum por dataset: Belvedere `0737ca650c6081b4` (1.343 bloques)
y Majadas `b830818c4aeddac0` (6.259 bloques). V7 redujo el tamaño total 11,77% en Belvedere
(1.591.650.185 → 1.404.306.523 B) y 38,77% en Majadas (7.533.781.717 → 4.612.644.109 B).
La mediana de conversión V7 fue 53,6% mayor en Belvedere (61,88 vs 40,30 s) y 40,3% mayor en
Majadas (517,71 vs 368,88 s); Majadas mostró rangos amplios, especialmente en V7 (444,70–787,20 s).
La descomposición atribuye el diferencial principalmente a preparación de bloques: mediana 186,89 s
en V7 frente a 58,23 s en V5; escritura ordenada fue 2,54 vs 1,66 s. Se cambió el ordenamiento
comparativo de pares `(clave espacial, fila)` por radix estable sobre la clave, conservando el orden
de filas duplicadas y sin cambiar candidatos ni bytes. Tres conversiones V7 instrumentadas de
Majadas bajaron la mediana de preparación a 173,77 s (-7,02%); el bloque completo bajó 7,07%, con
hashes SHA-256 idénticos entre repeticiones, mismo checksum lógico `b830818c4aeddac0` y pareja
idéntica de 4.612.644.109 B. La mediana total observada bajó 8,89%, pero una planificación espacial
de 482,63 s amplió el rango total a 460,77–689,42 s; RSS mediano subió 4,13% y el pico temporal
quedó prácticamente igual. Es progreso localizado, no cierre: V7 sigue con preparación ~2,98× V5
y total mediano ~27,9% mayor. Una prueba de reutilización del buffer radix no mostró ganancia
(181,55 s de preparación en una corrida) y se revirtió; no es evidencia concluyente de regresión.
Una comparación exploratoria de una corrida con dos workers dio V7 414,13 s frente a V5
311,95 s (+32,75%); la preparación fue 147,34 vs 45,72 s, con la misma pareja/checksum y RSS
232,6 vs 219,4 MB. Frente al V7 de un worker mediano, bajó el tiempo, pero no tanto como para
cerrar la brecha frente a V5. Es un dato único, no justifica cambiar defaults. Los archivos V5
locales antiguos llevan `OCTVWFRM` y el reader los
rechaza como `ProvisionalFormat`; las referencias válidas se regeneraron desde LAS/LAZ.
[Repeticiones V5/V7 y fases](../records/benchmarks/data/storage-v7-belvedere-majadas-v5-v71-2026-10-25.json),
[optimización radix y límites](../records/benchmarks/data/storage-v7-majadas-radix-index-2026-10-25.json).

Usar muestras versionadas y Belvedere, Albuera y Majadas disponibles, más casos sintéticos de
alta entropía/cardinalidad, constantes, extremos tipados, vacíos y duplicados. Fijar fuente,
opciones, binario/commit, perfil y hardware. Alternar repeticiones V5/pre4/V7 con misma precisión,
bloques y tareas; separar caché caliente/fría. No comparar hashes binarios entre formatos con
cabeceras distintas: comparar semántica y checksums lógicos, y hashes para determinismo por versión.

Medir pareja completa, bytes/punto, apertura/índice, XYZ/RGB/columna, solicitudes/bytes extra,
primer bloque y convergencia, pared/CPU, RSS/PSS y pico de disco temporal. La mejora de un
microbenchmark no certifica conversión ni navegación. Publicar dispersión y regresiones;
una pérdida de selectividad o un aumento de memoria puede invalidar el codec ganador en tamaño.

Verificar corrupción y truncamientos, límites antes de reservar, acompañantes incorrectos,
bloques preparados incompatibles, cancelación con trabajo activo, errores de productor/consumidor,
resultados obsoletos y recuperación/checkpoints. Nativo y WASM; Playwright al integrar la ruta
del visor, con datos reales, navegación, cambio de objetivo, cierre/reapertura y fallos parciales.

**Promoción hecha el 15-11-2026** ([ADR 0044](../adr/0044-v7-final-congelada-y-predeterminada.md), [plan D3c](d3c-congelado-v7-final.md)); queda la retirada de V5/V6 y de las identidades `-pre` (ver el inventario). Texto original: promover a V7 final/default después de demostrar contratos, recursos y latencia aceptables en
el corpus acordado, completar integración reader/writer y validar navegador. Regenerar muestras,
datasets activos y herramientas desde fuentes conservadas. Después retirar backends y opciones
V5/V6, manteniendo solo detección y error de regeneración cuando ayude al usuario. Archivar
especificaciones y benchmarks anteriores. Cierre: ninguna ruta de producción depende de V5/V6;
no basta con cambiar la etiqueta o el default.

### Inventario operativo para la promoción y retirada

| Ruta actual | Cierre que habrá que comprobar al promover |
| --- | --- |
| `viewer_convert::ConvertOptions`, CLI y `viewer_format::{version,layout,reader,writer}` | **Hecho:** V7 final es el default (ADR 0044). **Pendiente para la retirada:** V5 sigue siendo el único formato con `--checkpoint`/`--resume` y entrada EPT (V7.1–V7.3-pre también para EPT), así que retirarlo exige checkpoints en el formato paginado o una decisión sobre RF-26; V5/V6 y las `-pre` siguen como backends. Detección V5/V6 puede conservar un error explícito de regeneración; ningún payload legacy debe seguir necesitando su decoder. |
| `viewer_wasm/src/core_convert.rs` y `web-host/converter-worker.js` (el conversor antiguo `browser_convert.rs` se retiró en la etapa 4 del conversor web) | **Hecho** (D3c, [registro web](../records/benchmarks/D3C_WEB_BELVEDERE_15-11-2026.md)): constructor y worker escriben V7 final por defecto. Pendiente: retirar las opciones V5/V6 del selector. |
| `create_web_sample`, `web-host/sample*.oct*`, `package-local.py`, `data/samples/*.oct*` y datasets locales activos | **Hecho** para los datasets de render y las referencias ([registro](../records/benchmarks/D3C_V7_REFERENCES_15-11-2026.md)); `web-host/sample.oct*` pasa a V7 el 15-11-2026 ([ADR 0047](../adr/0047-v5-heredado-hasta-v8.md)); `sample-v5`/`sample-v7` quedan como heredados de las sondas, y `data/samples/*.oct*` sigue pendiente. Regenerar y validar antes de sustituir. Fuentes: generador sintético, LAS versionados de `data/samples/` y LAZ locales de Belvedere/Albuera/Majadas documentados en `data/datasets/README.md`. El paquete web debe incluir la pareja nueva. |
| Suites/perfiladores y generadores auxiliares | Herramientas activas aceptan V7; archivar comparaciones antiguas y retirar opciones/backend V5/V6 tras migrar los consumidores. |

La suite de almacenamiento y el perfilador ya comparten las revisiones soportadas, incluyendo
V7.1/V7.2; el comparador alternado usa V5/V7.2 por defecto. Los defaults de conversión eran V5; desde D3c son V7 (ADR 0044).
Pasan las 24 pruebas Python; la suite completó conversión V7.2 y lecturas hierarchy/reconstruct/
progressive sobre `fs_e430_n4008_18.las`. Esto habilita la matriz pendiente, no constituye promoción
ni retirada. [Comandos](../records/benchmarks/STORAGE_BENCH.md).
