# Evidencia del plan V7

Evidencia separada del [plan de formato V7](../../../../explanation/format-v7-plan.md).

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

[Smoke nativo sobre muestra real](data/storage-v7-pre1-smoke-2026-10-24.json):
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

[Smoke selectivo nativo](data/storage-v7-read-plans-smoke-2026-10-25.json): misma fuente,
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

[Smoke nativo de cuota compartida](data/storage-v7-shared-budget-smoke-2026-10-25.json):
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

## Evidencia del quinto incremento

El 25-10-2026 el worker pasó a abrir el reader de Blob fuera de la cola de mensajes y a pedir
al viewer el bloque pendiente de mayor prioridad LOD. Las lecturas de geometría y atributo usan
`planRead`/`read` con `Blob.arrayBuffer`; la inserción y publicación quedan en la cola síncrona
WASM. Una lectura en vuelo como máximo mantiene acotada la cola. El worker compara `peak_bytes`
y los resultados retenidos con `cacheBytes` antes de continuar una lectura; el test rechaza un
plan sobredimensionado antes de leer. Esa cuota limita los
buffers lógicos del trabajo, pero aún no agrega apertura, caché y resultado insertado en un solo
presupuesto de memoria.
La generación del dataset/cámara/objetivo invalida el resultado al completarse; `Blob.arrayBuffer`
no es abortable aquí, por lo que la cancelación es lógica. Los fallos dejan el bloque pendiente y
requieren una nueva refinación explícita para reintentar.

El worker conserva XYZ, RGB y columna tipada; los modos intensidad/clasificación solicitan su
columna por nombre de schema. `openLodDataset` recibe visualScale, jerarquía, semántica, esquema,
rango y disponibilidad del companion. La lectura no retiene un préstamo de WASM durante `await`.
Node cubre los cambios de color, estadísticas, respuesta de frame durante lectura demorada y
descarte tras cambiar la generación. `cargo check` y Clippy WASM, formato y diff pasan. El probe
real del navegador pasa contra `sample-v7.oct`/`.octattrs`: 25 rangos Blob, geometry, XYZ, RGB,
U16 y rechazo CRC.

No se pudo probar la integración visual del worker en este navegador: la inicialización WebGPU
respondió `no GPU adapter supports this surface`. Tampoco se implementan aún la cuota agregada de
apertura, caché, lecturas y resultados, concurrencia mayor que uno, deduplicación,
reintento/cancelación física ni medición de RAM/latencia durante navegación. Por tanto la prioridad
3 sigue abierta y esta entrega no valida rendimiento o continuidad visual GPU.

## Evidencia del sexto incremento

El 25-10-2026 cada lectura LOD ahora tiene un request WASM independiente, respaldado por el mismo
índice inmutable y una cuota RAII compartida. La apertura reserva también el tamaño del índice
residente. Cada reserva cubre el pico planificado, buffers Blob/WASM y salida tipada hasta que el
worker inserta el bloque o descarta el resultado obsoleto. Las lecturas usan
`Blob.slice(...).arrayBuffer()`; cancelar una generación es lógico y conserva cuota hasta que las
promesas activas terminen.

El worker toma el orden completo de bloques pendientes del plan LOD y solapa hasta cuatro
peticiones. Si una petición no cabe por reservas activas, permanece pendiente; se prueban bloques
posteriores y el conjunto vuelve a intentarse cuando una lectura libera su reserva. Los fallos
reales siguen requiriendo una refinación explícita para reintentar. El scheduler de tickets de
`viewer_core` todavía no se integra en WASM, y cuatro es el límite local actual, no un valor
ajustado mediante mediciones.

Node cubre solapamiento de tareas, disponibilidad del worker durante lecturas, descarte de
generación obsoleta, diferimiento por cuota y rechazo de una lectura individual mayor al límite.
La suite web completa pasa (36 tests); las pruebas nativas de `viewer_wasm` pasan (34; 1
microbenchmark ignorado), y check/Clippy/formato WASM pasan.
El probe real de navegador pasa: dos solicitudes independientes del mismo reader alcanzan más de
una llamada Blob activa y conservan XYZ, RGB y U16; todas las reservas vuelven al nivel de metadata
tras liberar resultados. Comprueba checksum, rangos Blob y companion. Esta prueba no necesita GPU.
La inicialización visual del worker sigue sin estar validada en el navegador disponible por
`no GPU adapter supports this surface`; tampoco hay medida de latencia/RAM con navegación LOD.

Quedan abiertos en la prioridad 3: integrar el scheduler/tickets y retry explícito, definir el
presupuesto conjunto con caché WASM/GPU, probar sustitución del dataset con varias lecturas
pendientes en navegador y medir latencia/memoria durante navegación real.

## Evidencia del séptimo incremento

El scheduler WASM envuelve `BlockRequestScheduler`: actualiza selección y prioridad desde el orden
del plan LOD, emite tickets opacos, informa cancelaciones, valida tickets vigentes antes de insertar
y conserva los fallos hasta un retry explícito. La admisión por bytes sucede antes de tomar un slot;
una lectura sobredimensionada se marca fallida sin iniciar Blob. Al cancelar una lectura activa el
slot permanece ocupado hasta que su promise termina y se completa el ticket.

El worker reconcilia el scheduler tras cada cambio LOD, inserta solo resultados con ticket todavía
vigente y cancela lógicamente los obsoletos. Un cambio de color vacía tickets activos para releer
con la columna solicitada. Una refinación permite reintentar los bloques fallidos; la sustitución
del dataset cancela tickets anteriores y usa un scheduler nuevo.

`cargo test -p viewer_core --offline` pasa (31 unitarias y 75 de integración); check/Clippy WASM,
formato, 37 pruebas Node y el probe navegador con scheduler WASM pasan. El probe cubre prioridad
entre candidatos admitidos, límite de slots, cancelación, descarte y reintento. La integración
visual del worker sigue sin comprobarse por falta de adaptador WebGPU en el navegador disponible;
Node además prueba la sustitución del dataset mientras dos lecturas están abiertas: conserva los
slots viejos hasta completarse, descarta esos resultados y luego admite los dos bloques nuevos.
Esta ruta usa un viewer simulado y no prueba la integración visual real. No se ha medido
memoria/latencia real.

La prioridad 3 aún requiere navegación GPU real, prueba del worker completo sustituyendo dataset
con varias lecturas Blob activas y memoria conjunta con caché WASM/GPU. El límite actual de cuatro
lecturas sigue provisional y no está basado en una comparativa de recursos y latencia.

## Candidato de conversión V7.2-pre

Para reducir CPU del writer se conservan dos identidades: V7.1-pre mantiene su búsqueda completa
(24/28 bits, ocupación heredada y compacta); V7.2-pre evalúa solo ocupación compacta de 24 bits
frente a raw/delta. Ambas comparten decodificación, mantienen orden de filas y los mismos valores
lógicos, pero V7.2 usa magias de índice/footer propias. Así los resultados no se confunden con la
línea base y los checkpoints EPT permanecen ligados a una revisión exacta. La alternativa sencilla
era cambiar la selección bajo V7.1; se descarta porque habría hecho dependientes del binario los
bytes nuevos de la misma identidad.

La señal preliminar justifica medir el candidato: en Majadas, compact-only fue byte a byte igual
al selector de 24 bits con serializaciones redundantes (+763 B respecto a V7.1 completo), y la
preparación observada bajó de 173,77 s (mediana V7.1) a 126,39 s en una corrida. En Albuera,
compact-only también coincidió byte a byte con ese selector de 24 bits; preparación 210,86 s
frente a 259,87 s de la corrida V7.1, con +52.451.191 B. Estas pruebas temporales todavía tenían
marcador V7.1 y una de las duraciones totales fue atípica; no se consideran medición formal de
V7.2 ni criterio de promoción. La diferencia pequeña de tamaño en Albuera y la latencia absoluta
V7 frente a V5 obligan a completar perfiles identificados como V7.2 antes de decidir.

**Cierre implementado y medido el 25-10-2026:** V7.1 conserva su selector; V7.2 usa identidad
exacta propia, y lectura nativa/async mono/split. EPT V7.2 retoma tras un bloque confirmado y
coincide byte a byte con una ejecución limpia; los checkpoints de V7.1 no se aceptan como V7.2.
Perfiles nativos identificados como V7.2 y lector progresivo confirmaron los checksums V5 en ambos
datasets. Las cifras, hashes, contadores de candidatos y límites están en
[el perfil compact-only](data/storage-v7-v72-compact24-2026-10-25.json).

Majadas: preparación 126,52 s frente a la mediana V7.1 de 173,77 s (-27,2%); total 447,89 s
frente a 471,70 s (-5,0%); pareja +763 B y 8 bloques compactos. Albuera: preparación 202,41 s
frente a la corrida V7.1 de 259,87 s (-22,1%); total 670,81 s frente a 796,84 s (-15,8%); pareja
+52.451.191 B y 611 bloques compactos. Una muestra por dataset no aísla la variación de
planificación; V7.2 sigue experimental. Aun así, tarda 21,4% más que V5 histórico en Majadas y
40,3% más en Albuera, aunque las parejas sean menores; la optimización parcial no satisface el
criterio de promoción. Repetir con orden
controlado y separar planificación/preparación antes de aceptar un cambio de default.

## Claves Morton compactas para ocupación superficial

El índice de celdas ocupadas ahora ordena claves `(u64, fila)` cuando la profundidad del octree
necesita como máximo 64 bits (hasta profundidad 21); las raíces más profundas conservan
`(u128, fila)`. Las claves V7.2 de 24 bits usan profundidad 8, por lo que se reduce a dos tercios
el ancho de cada entrada de ordenación y su buffer scratch. La alternativa más sencilla era dejar
siempre `u128`; las medidas mostraron una mejora pequeña con el mismo orden, pareja y checksum.

En Belvedere V7.2, una corrida previa y tres posteriores mantuvieron 3 bloques compactos, 260
delta y 1.080 raw; la mediana posterior fue 18,22 s de preparación y 51,37 s total, frente a
19,13 s y 53,07 s en la corrida anterior (-4,8% y -3,2%). Los hashes de ambas salidas coinciden
entre la base y las corridas posteriores verificadas; el lector progresivo confirmó
`0737ca650c6081b4`. El RSS mediano posterior fue 7,3% mayor, mientras los temporales quedaron
prácticamente iguales. Un A/B de una corrida en Majadas también baja preparación de 126,52 a
120,45 s con la pareja idéntica; el total cae más porque la planificación fue 55 s menor, así que
no se atribuye al sort. Falta una repetición de Majadas y medir Albuera. El detalle queda en
[el A/B de claves Morton](data/storage-v7-v72-shallow-morton-keys-2026-10-25.json); la
optimización mantiene los bytes V7.2 y no altera el estado experimental del formato.

## Diagnóstico optativo del coste de candidatos geométricos

La preparación V7 probaba geometría raw, delta y ocupación comprimida para elegir el menor
payload. No se debe podar un candidato solo porque rara vez gane en una muestra: los perfiles
previos cuentan 611 bloques compactos en Albuera y solo 3 en Belvedere. Se añadió instrumentación
optativa de intentos, bytes, tiempo de encode/compresión y ganadores por candidato; el tiempo común
de construir `QuantizedOccupancy` se agrupa aparte por profundidad. Sin diagnóstico, el encoder
mantiene la ruta normal sin temporizadores ni asignaciones de métricas. Las salidas con/sin métricas
se comparan byte a byte para V7.1-pre y V7.2-pre.

Un perfil por dataset con un worker registró en Belvedere 1.343 intentos compactos de 24 bits y 3
selecciones; la construcción de ocupación sumó 2,82 s y encode+compresión compacta 0,98 s. En
Albuera hubo 8.437 intentos y 611 selecciones; construcción 20,29 s y encode+compresión 6,16 s.
El delta consumió 1,28 s en Belvedere y 10,35 s en Albuera; raw, 0,28 s y 2,37 s. Los hashes de
ambas parejas coinciden con las salidas V7.2 previamente validadas y leídas progresivamente; no
cambió el checksum lógico. La conversión profileada totalizó 52,66 s en Belvedere y 509,99 s en
Albuera, pero es una sola corrida instrumentada por fuente y no un A/B de rendimiento. Los tiempos
por candidato son sumas de trabajo; pueden solaparse entre workers. Cada corrida usó un worker,
por lo que aquí no se solaparon. RSS y temporales son muestreados. No se elimina ni altera ningún
candidato; el siguiente experimento debe optimizar construcción/codificación y comprobar el coste
de tamaño con las salidas completas. [Datos por candidato](data/storage-v7-v72-candidate-costs-2026-10-25.json).

## Filas estrechas para el radix Morton

En ocupación de profundidad máxima 10, la clave Morton cabe en 30 bits. Si el iterador ofrece un
límite superior que prueba que las filas caben en `u32`, el índice ahora ordena pares `(u32, u32)`;
para claves/filas mayores conserva `(u64, usize)` o `(u128, usize)`. En el heap de 64 bits reduce
de 32 a 16 bytes por fila entre entrada y scratch del radix. La cota del iterador impide truncar
filas y el orden estable conserva duplicados/asociación. La topología y payload V7 no cambian.

Tres parejas V7.2-pre alternadas en Belvedere mantuvieron los mismos hashes de salida. La mediana
de preparación pasó de 18,38 a 18,21 s (-1,0%); construcción de ocupación, 2,78 a 2,59 s (-6,7%);
total, 52,86 a 52,47 s (-0,7%). El RSS mediano subió de 105,0 a 107,4 MiB y el pico temporal
muestreado de 4,638 a 4,645 GB, así que no se declara una reducción observada de RSS/memoria.
En Albuera N=1, la construcción pasó de 20,29 a 17,70 s (-12,8%) y ambas salidas tuvieron hashes
idénticos. La preparación total pasó de 220,76 a 180,97 s, pero también cambiaron los tiempos
medidos de raw/delta y otras fases; esa diferencia no se atribuye al radix. Este es un ahorro
pequeño y acotado, sin promoción de V7.2. El detalle y rangos están en
[la comparación de filas estrechas](data/storage-v7-v72-shallow-morton-rows-2026-10-25.json).

### Intercalado Morton por máscaras: descartado

Se probó reemplazar el bucle bit a bit por expansión de máscaras para profundidades de hasta 10,
con igualdad comprobada contra la referencia para profundidades 0–10. Tres pares alternados en Belvedere
conservaron los hashes, pero la construcción de ocupación solo bajó 2,0%; la preparación subió
1,9% y el total 0,4%. El RSS y el pico temporal muestreados fueron algo menores, sin repetición
representativa para atribuirlo. Se revirtió la ruta especializada: el coste de código no se justifica
sin mejora de conversión. [Datos y rangos](data/storage-v7-v72-morton-mask-rejected-2026-10-25.json).


## Claves enteras para diccionarios de atributos

Los tiempos de geometría no explican toda la preparación; RGB y columnas tipadas también prueban
un diccionario antes de elegir raw/LZ4/planos. Para tuplas de 2–8 bytes, el mapa ahora usa claves
`u64` construidas con cargas de tamaño fijo y padding cero. El ancho es constante por diccionario:
la clave conserva todos los bits, incluidos NaN y cero con signo. Se mantiene el `HashMap` estándar,
la paleta de primera aparición, IDs, desempates y bytes comprimidos. Anchos mayores conservan
claves de slices y el ancho 1 mantiene su lookup directo. La construcción común de IDs se comparte.
La alternativa de copiar una longitud variable a un array empeoró varios anchos y se descartó.

La comparación diferencial cubre anchos 2–9/12/16, vacíos, constantes, cardinalidad y patrones
binarios; compara codec y payload completo con el encoder anterior. El ensayo aislado `perf`,
262.144 filas y tres repeticiones por ancho/cardinalidad, observó entre 16% y 38% menos tiempo.
Eso no equivale a una mejora global: tres parejas alternadas V7.2 en Belvedere bajaron la mediana
de preparación 17,84→17,38 s (-2,6%) y total 52,68→51,96 s (-1,4%). Los seis pares conservaron
los SHA-256 conocidos; RSS pico muestreado mediano 111.538.176→105.492.480 B (-5,4%) y temporales
prácticamente iguales. Un corpus y un worker, caché sin purgar; faltan Albuera/Majadas y la matriz
actual V5/V7. No cambia el estado experimental de V7.2. [Opciones, rangos y datos](data/storage-v7-v72-integer-dictionary-2026-10-25.json).

Pasan formato/Clippy, `viewer_format --all-targets --features async-reader`, las 62 pruebas de
`viewer_convert --lib` (incluida recuperación EPT) y compilación WASM. Siguiente cierre: atribuir
coste/beneficio de candidatos de atributos en corpus grandes antes de cambiar su selección;
conservar esta mejora de lookup sin inferir que resuelve la brecha total con V5.


## Coste y beneficio de diccionarios por columna

Se regeneró Albuera con V7.2 actual: 455,78 s total, 169,83 s de preparación, pareja de
10.373.185.958 B y los SHA-256 ya conocidos. Es N=1 y no permite comparar latencia con V5.
Una sonda ignorada en `attribute_codec` lee un payload por vez y selecciona IDs repartidos por el
índice, incluyendo extremos. Excluye lectura/serialización de tiempos y alterna tres encodes por
payload. En el perfil final comprueba codec, longitud y CRC del encoder base contra lo persistido,
además de reconstrucción exacta en ambas alternativas. Se ejecuta con ruta absoluta en
`VIEWER_ATTRIBUTE_PROFILE_PATH`, límite `VIEWER_ATTRIBUTE_PROFILE_BLOCKS` y
`cargo test --profile perf -p viewer_format profile_attribute_encoders_on_split_corpus -- --ignored --nocapture`.

Los 512 bloques de Albuera suman 29.011.991 filas incluyendo LOD. Eliminar diccionarios de todas
las tuplas anchas cambia payloads de atributos de 244.808.170 a 251.222.659 B (+2,62%) y el encoder
medido de 3,84 a 1,29 s (-66,4%). Intensidad, ángulo F32 y source ID aportan gran parte del ahorro
de bytes del diccionario, por lo que no conviene eliminarlos por ser columnas numéricas.
Conservar candidatos de diccionario en tuplas de **1, 2 y 4 bytes**, usando la alternativa barata en
las demás, compone 245.260.314 B (+0,185%) y 2,05 s (-46,5%) sobre esos mismos payloads. Es una
composición de mediciones por columna, no una conversión nueva ni un ratio de pareja completa.
El perfil inicial de 128 bloques apuntó al mismo intercambio. [Datos, codecs y límites](data/storage-v7-v72-attribute-candidates-2026-10-25.json).

**Siguiente candidato:** V7.3-pre con identidad propia, geometría V7.2 y diccionarios solo para
anchos 1/2/4; raw/LZ4, constantes y planos permanecen disponibles. V7.1/V7.2 conservan sus bytes.
Implementar y verificar identidad/lecturas selectivas, nativo/async/WASM y reanudación EPT exacta;
luego convertir corpus completos y contrastar tiempo/tamaño/recursos antes de promover. La pareja
Albuera generada se conserva temporalmente como control para esa comparación y se limpiará al
cerrarla. La sonda no cambia ninguna ruta de producción. Pasan su prueba de corrección, el perfil
con CRC de 512 bloques, Clippy y formato.


## Implementación del candidato V7.3-pre

Implementado con [identidad propia](../../reference/format/FORMAT_V7_PRE3.md): major 7/revisión 3,
índice `OCTVI73P` y footer `OCTVE73P`. La geometría mantiene compact24/raw/delta de V7.2; solo se
prueban diccionarios de atributos con tuplas de 1/2/4 bytes. El resto conserva raw/LZ4, constantes
y planos. El encoder comparte las reglas raw/constante/planos para conservar sus desempates; las
pruebas diferenciales y el recodificado de tres bloques Albuera comprueban los bytes V7.2 previos.
V5 sigue default; V7 final permanece rechazado.

Pasan tests `viewer_format --all-targets --features async-reader`, `viewer_convert --all-targets`,
Clippy, check/build WASM y las 24 pruebas Python. Se verifican mono/split y async, marcadores y
companions incompatibles, bits tipados, salida serial/paralela/pipeline y recuperación LAS. EPT
reanuda V7.2/V7.3 tras un bloque confirmado con salida igual a la limpia y rechaza otra revisión
sin tocar ninguno de los dos prefijos de la pareja.

Playwright MCP/Chromium 156 ejecutó el reader Blob async con fixture V7.3 de 4.096 puntos (29
rangos/arrayBuffers, XYZ/RGB/U16, CRC, cuota y tickets/retry); consola sin errores. También ejecutó
el worker de conversión con 111.190 puntos LAS, handles síncronos OPFS reales y selección V7.3;
reabrió la pareja, comprobó conteos/esquema y rechazó companions incorrectos. Eliminó sus archivos
OPFS. La suite nativa completó conversión V7.3 y hierarchy/reconstruct/progressive con la misma
fuente pequeña. [Artefactos y límites](data/storage-v7-v73-integration-2026-10-25.json).
Son pruebas de integración de almacenamiento, no presentación GPU ni latencia de navegación.

**Pendiente de medición:** convertir Belvedere/Albuera/Majadas y contrastar V7.2/V7.3 y V5 en
procesos identificados/alternados, con checksums lógicos, pareja completa, tiempos por fase,
RSS/temporales y lectura selectiva. El +0,185%/−46,5% del muestreo de atributos no se considera
resultado de la conversión nueva. V7.3 sigue optativo; todavía no se promueve ni retira V5/V6.
