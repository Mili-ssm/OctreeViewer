# V7: almacenamiento y ejecución acotada

Decisión del 24-10-2026: dedicar el desarrollo al sucesor V7 y retirar V5/V6 al completar
su sustitución. El [README](../README.md#prioridades-pendientes) registra el progreso activo.
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

No se han acordado mínimos de hardware, almacenamiento objetivo ni tolerancias numéricas de
regresión de latencia. Se puede avanzar con comparaciones relativas y corpus local existente;
cerrar esas decisiones antes de congelar el formato, sin inventar un porcentaje de mejora.

## Base contrastada

| Base inspeccionada | Conservar / corregir |
| --- | --- |
| [V5](../crates/viewer_format/FORMAT_V5.md): separación `.oct`/`.octattrs`, columnas y CRC independientes. | Abrir XYZ sin acompañante; RGB o una columna sin leer las demás; no compartir IDs entre columnas por defecto. |
| [V6 pre4](../crates/viewer_format/FORMAT_V6_PRE4.md): delta, ocupación compacta, constantes, diccionarios y planos de bytes con fallback. | Reutilizar codecs y validaciones; contrastar tamaño completo y coste de candidatos. La ocupación por bloque no define una grilla global. |
| [I/O existente](STORAGE_V6_IO.md): preparación Rayon, pipeline optativo con hasta dos lotes, `fork_reader` e índice compartido. | Reutilizar límites de propiedad y escritura ordenada; límites por lote y modelos actuales no garantizan RSS. La lectura paralela del ejemplo no está integrada en el viewer. |
| [`BlockRequestScheduler`](../crates/viewer_core/src/scheduler.rs) y [reader](../crates/viewer_format/src/contracts.rs). | Prioridad, tickets y cancelación lógica existentes; falta integración WASM y admisión agregada por bytes. |
| [Memoria de conversión](V5_MEMORY_BENCH.md) y [memoria web](WEB_PERFORMANCE_BENCH.md). | Separar spools, buffers, índice, caché, WASM y GPU; las medidas existentes pertenecen a sus versiones y entornos. |

## 1. Base V7 verificable

Primera revisión optativa [`v7.1-pre`](../crates/viewer_format/FORMAT_V7_PRE1.md), con identidad
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
pipeline y checkpoints con identidad exacta; V5 sigue siendo el default operativo durante
la transición. La conversión web y sus defaults no cambian en este incremento.

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

**Pendiente de integración async/visor:**

**Decisión confirmada por el usuario (25-10-2026):** la ruta WASM debe obtener apertura e índice
y payloads mediante `Blob.slice(...).arrayBuffer()` u otra API Blob realmente asíncrona. No se
considera cumplimiento envolver `FileReaderSync` en una tarea o worker.

**Primer incremento implementado:** `FormatReader::materialize_selected_from_buffers` consume
un `ReadPlan` de la misma instancia lógica y los buffers stored de sus rangos. Valida identidad,
cantidad, longitudes y CRC antes de reutilizar los decodificadores de codec, XYZ, RGB y atributos;
no hace I/O de archivo ni cambia los bytes V7. Las pruebas comparan la lectura síncrona para
posiciones, geometría con/sin atributos, RGB y una columna, y rechazan plan ajeno, buffer truncado
y checksum incorrecto. Esto habilita decodificar rangos externos, pero aún no implementa apertura
Blob asíncrona, admisión por bytes ni integración con el ciclo LOD.

Conectar el scheduler del core al controlador LOD con selección, prioridad, identidad de dataset
y generación de objetivo. Limitar solicitudes y bytes simultáneamente, incluidos resultados.
El renderer consume escenas; no abre archivos. Abrir el índice del par split de forma asíncrona
con límites `FormatReaderLimits`; luego reservar memoria antes de solicitar cada rango. Incluir
en la cuota los `ArrayBuffer` JS, su copia a WASM, scratch de decodificación y resultado retenido.

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

Medir disco simultáneo: spools fuente/hijos/LOD + candidatos `.oct`/`.octattrs` + pareja anterior
durante sustitución + checkpoints conservados. Comparar liberación temprana del último rango
con spools comprimidos/chunks: menos bytes puede exigir más CPU y perjudicar lecturas parciales.
Elegir según medidas de tráfico y pico; no cambiar checkpoints sin versionarlos.

**Siguiente cierre medible:** establecer una base V7 en Belvedere, Albuera y Majadas para 1/2/4
workers y con/sin checkpoint; incluir EPT con y sin `--pipeline-writes`: repetir tiempos, RSS/PSS
muestreados, bytes temporales máximos, hashes y capacidades retenidas de lotes. El profiler de
filesystem puede perder picos breves o spools eliminados durante el recorrido; sus muestras no son
una cota exacta de disco físico. Usar esa base antes de reemplazar la estimación de memoria actual
por admisión ponderada por bytes, y aceptar el cambio solo si reduce el pico medido sin cambiar
salida, orden ni recuperación.

**Primera línea base V7 a escala:** una ejecución `perf`, sin checkpoint, un worker, divisiones 2
y 262.144 puntos/bloque convirtió Albuera (`SHA-256` de fuente en el JSON). La pareja V7.1-pre
ocupa 10.320.734.767 B frente a 13.666.843.763 B de la última V5 guardada con igual fuente y
opciones: **3.346.108.996 B menos (24,48%)**. Los dos informes coinciden en 8.437 bloques,
508.809.556 puntos fuente, 179.791 LOD y 20.868.563.227 B de payload lógico. El recorrido
`storage_bench progressive` verificó las dos salidas con el mismo checksum lógico BFS
`8caae2a09753e50e`: incluye IDs, coordenadas/color reconstruidos, nombres/tipos de columnas y
valores tipados. Esto verifica igualdad lógica completa para esta pareja.

La muestra V7 observó 122.630.144 B de RSS máximo y 27.459.415.690 B de temporales simultáneos;
la V5 anterior registró 111.386.624 B y 27.436.877.498 B. El tiempo fue 608,46 s frente a
448,87 s, pero son una corrida por versión en días distintos, binario V5 no identificado y carga
de host/caché no controlada: no atribuir el tiempo ni la variación de RSS al formato. La lectura
completa posterior tardó 59,17 s en V5 y 64,18 s en V7; se ejecutó una vez, V5 primero y sin
vaciar caché, así que tampoco es evidencia comparativa de latencia. Los tamaños, checksums y picos
muestreados están en [la evidencia Albuera V5/V7](benchmarks/storage-v7-albuera-writer-smoke-2026-10-25.json).

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

Promover a V7 final/default después de demostrar contratos, recursos y latencia aceptables en
el corpus acordado, completar integración reader/writer y validar navegador. Regenerar muestras,
datasets activos y herramientas desde fuentes conservadas. Después retirar backends y opciones
V5/V6, manteniendo solo detección y error de regeneración cuando ayude al usuario. Archivar
especificaciones y benchmarks anteriores. Cierre: ninguna ruta de producción depende de V5/V6;
no basta con cambiar la etiqueta o el default.

## Evidencia del primer incremento

Implementado el 24-10-2026: `v7.1-pre` en reader/writer y CLI, herencia de capacidades pre4,
identidad exacta y reducción de buffers auxiliares. Las matrices de conversión incluyen V7
para EPT, salida serial/pipeline y reanudación de validación/checkpoint incompatible.
El cambio de buffers tiene pruebas diferenciales contra la construcción anterior y un oráculo
de empaquetado por bits. Las pruebas V7 incluyen lecturas selectivas, bits tipados, diccionario,
constante, planos de bytes, ocupación compacta mono/split, compañeros y bloques preparados
incompatibles, forks, CRC y rechazo de V7 final/revisiones futuras antes de tocar los sinks.

Comprobaciones ejecutadas:

- `cargo test-fast --offline`: workspace nativo, incluidas las regresiones V7. Las pruebas
  ignoradas de GPU/microbenchmarks siguen sin ejecutar; esto no prueba presentación web.
- `cargo test -p viewer_format --offline --all-targets` y
  `cargo test -p viewer_convert --offline --all-targets`: alcance de formato y conversor.
- `cargo clippy --workspace --all-targets --offline -- -D warnings` y
  `cargo clippy -p viewer_wasm --target wasm32-unknown-unknown --offline -- -D warnings`.
- `cargo fmt --all -- --check`, `git diff --check` y destinos relativos de la documentación.

[Smoke nativo sobre muestra real](benchmarks/storage-v7-pre1-smoke-2026-10-24.json):
`fs_e430_n4008_18.las`, bloques objetivo de 16.384 puntos, perfil `fast` sin optimizar.
Conversión pre4 con pipeline, V7 serial y V7 con pipeline; lectura BFS por dos lectores del
ejemplo. Las tres lecturas recorren 23 bloques y 111.422 filas **incluido LOD**, checksum lógico
`6addac9995280fc1`. Los recuentos totales se corrigieron el 25-10-2026: el script original
había sobrescrito el resumen con el último nivel BFS; la corrección y su verificación constan
en el JSON. Las dos parejas V7 son idénticas byte a byte. Pre4 y V7 ocupan ambos
1.569.368 B; sus hashes de archivo difieren por identidad. El JSON conserva comandos y hashes
de fuente/binarios/salidas; `$OUT` representa un directorio temporal nuevo para reproducirlos.

No se ha medido una reducción de RSS, disco temporal ni velocidad de V7. No se ha ejecutado
V7 en navegador ni regenerado bindings/paquete web: el check WASM es estático. La UI no cambia
en ese primer incremento. El segundo añade planificación selectiva; la admisión concurrente
por bytes y el transporte async siguen pendientes.

## Evidencia del segundo incremento

Implementado el 25-10-2026: planes selectivos sin I/O y preflight por lectura sobre revisiones existentes,
sin cambiar los bytes de V7.1-pre ni necesitar otra revisión física. Las pruebas contrastan
rangos y bytes reales en V5/pre4/V7, split y monolítico, y conservan resultados/bits tipados.
Incluyen presupuesto exacto frente a `pico-1`, desconocidos, acompañante ausente, overflow,
ocupación, diccionario y planos de bytes. Pasan `cargo test-fast --offline`, los tests de formato
con `--all-targets`, los 7 tests del ejemplo `real_dataset_bench`, Clippy del workspace con
`--all-targets`, Clippy WASM, formato y diff. Los tests ignorados de GPU/microbenchmark no se
han ejecutado; el check WASM no es ejecución en navegador.

[Smoke selectivo nativo](benchmarks/storage-v7-read-plans-smoke-2026-10-25.json): misma fuente,
opciones y hashes de salida V7 del primer incremento, perfil `fast` sin optimizar. Se recorre
el catálogo de 23 bloques y 111.422 filas incluidas las de LOD. XYZ lee 1.091.188 B del principal
y cero atributos; la columna 0 lee 137.525 B de atributos y cero geometría. Ambos coinciden
exactamente con sus planes. Los mayores picos **estimados de buffers** son 391.963 B para XYZ
y 99.967 B para esa columna; no son RSS medido ni una comparación de rendimiento.

Al cerrar históricamente ese segundo incremento quedaban pendientes cuota compartida y transporte;
el tercero añadió reservas y ejecución paralela nativa. El muestreo adicional de temporales/RSS
sigue pendiente.


## Evidencia del tercer incremento

Implementado el 25-10-2026: cuota compartida, propiedad de resultados y ejecución selectiva
paralela nativa. No cambia el layout ni los bytes de V7.1-pre. Los checks y comparaciones
nativas de esta entrega se registran a continuación; el visor WASM sigue sin usar esta ruta.

`cargo test-fast --offline`, `cargo test -p viewer_format --offline --all-targets`, Clippy del
workspace con `-D warnings`, Clippy WASM, formato y diff pasaron. Las pruebas cubren reservas
exactas, resultados retenidos, CRC/I/O, fallos de preflight, solapamiento real de workers,
orden de entrega, límite de ventana/cuota, callback, panic y reanudación. No hubo cambios de
layout ni de bytes de V7.1-pre.

[Smoke nativo de cuota compartida](benchmarks/storage-v7-shared-budget-smoke-2026-10-25.json):
24 configuraciones (XYZ, geometría, RGB y atributo; 1/2/4 lectores; límites 1/64 MiB) sobre la
muestra real y los 23 bloques/111.422 filas incluidos LOD. Para cada selección, los checksums
coinciden entre configuraciones; todas terminan con cero bytes reservados y picos dentro del
límite. El pico lógico XYZ con cuatro lectores es 841.093 B para ambos límites. Los tiempos son
una ejecución por caso en perfil `fast` sin optimizar; el propio JSON declara que no son evidencia
de speedup ni de RSS.

## Evidencia del cuarto incremento

El 25-10-2026 se añadió la feature nativa optativa `viewer_format/async-reader`, con Tokio
1.53.3. `AsyncFormatReader` abre y valida una pareja de archivos una vez; clones del handle
comparten estado lógico, slots y cuota. Unix usa lecturas posicionadas sobre handles compartidos;
otras plataformas tienen fallback de `seek/read` serializado por archivo. La aplicación del
viewer aún deberá mantener una sola instancia por dataset y conectar su scheduler/LOD; la
clonabilidad pertenece a la biblioteca. Caché, coalescing/deduplicación, prefetch y transporte
WASM siguen pendientes.

Las cuatro pruebas async cubren lectura de XYZ/geometría/RGB/atributo con fixture monolítico y
split, solapamiento de dos workers, cuota retenida por el último `Arc` y wakeup al liberarla,
y cancelación del future con un worker activo conservando slot/cuota hasta terminar. Pasaron
`cargo test -p viewer_format --all-targets --features async-reader --offline` (90 tests de
unidad, 3 de integración y targets de ejemplos; 1 microbenchmark ignorado),
`cargo test-fast --offline`, Clippy del workspace y de `viewer_format` con feature, Clippy WASM,
`cargo fmt --all -- --check` y `git diff --check`.

Esta verificación es nativa y usa fixtures; no mide RSS, latencia/throughput comparativos ni
valida integración de aplicación/navegador. El límite es el modelo lógico planeado y no un techo
de RAM de proceso. La lectura bloqueante/decode se ejecuta fuera del executor async en el pool
bloqueante; la API no certifica I/O nativo no bloqueante.
