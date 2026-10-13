# Benchmarks formales de almacenamiento

Protocolo añadido el 13-10-2026 para preparar comparaciones v5/v6. Están soportados v5 definitivo y el experimental v6.1-pre (`OCTVFRM1`, envelope 1); las series provisionales
se conservan separadas y no certifican el formato final.
No confundir reconstrucción del índice persistido con recalcular una jerarquía desde LAS ni con
construir vóxeles/buffers GPU. El nuevo benchmark no cambia la interfaz web.

## Cobertura y herramientas

| Pregunta | Herramienta / métrica | Alcance |
| --- | --- | --- |
| ¿Cuánto ocupa? | `real_dataset_bench convert-profile`: `geometry_file_bytes`, `attribute_file_bytes`, `output_size_bytes` | Archivos finales completos, incluidos índices, CRC y overhead; no sólo payload comprimido. |
| ¿Cuánto tarda producir los archivos? | `conversion_elapsed_ms` | LAS/LAZ → pareja validada/publicada; incluye planificación, lectura, compresión, escritura y validación. |
| ¿Cuánto corresponde a escribir? | `internal_ordered_write_and_release_elapsed_ns`, nuevo `internal_finalize_and_sync_elapsed_ns` | Primero: escrituras ordenadas, liberación de spools y callbacks. Segundo: cierre índice/footer y `sync_all` de ambos candidatos. No son latencia de disco pura; excluyen publicación/directorio. Preparación/compresión se mide aparte. |
| ¿Cuánto tarda recuperar la estructura completa? | `storage_bench … hierarchy`: `open_ms` | Apertura y validación de cabecera, footer, índice y jerarquía; sin payload ni acompañante. |
| ¿Cuánto tarda reconstruir todos los datos residentes? | `storage_bench … reconstruct`: `load_ms`, `full_ms` | Abre índice, adjunta acompañante y retiene todos los `PointBlock` decodificados, incluidos representantes LOD. Presupuesto previo obligatorio. |
| ¿Cuánto cuesta cargar hijos secuencialmente? | `storage_bench … progressive`: `first_root_ms`, `load_ms`, `node_p50_ms`, `node_p95_ms` por nivel | Recorrido BFS raíz→hijos en orden estructural estable; bloques del nodo secuenciales, un bloque decodificado vivo. Sin caché de render, red ni GPU. |

Ya existían conversión/tamaños, lecturas secuenciales por BlockId, aleatorias y muestras en
`real_dataset_bench`; siguen siendo útiles y no equivalen a recorrer padres/hijos. Véanse
[las mediciones v5 anteriores](V5_MEMORY_BENCH.md). Se reutiliza esa conversión, añadiendo
instrumentación de cierre/sync, el ejemplo de reconstrucción/progresión y un runner conjunto.

## Ejecución reproducible

Compilar una vez, fuera de las mediciones:

```sh
cargo build --profile perf --offline -p viewer_convert --example real_dataset_bench \
  -p viewer_format --example storage_bench
python3 crates/viewer_convert/benchmarks/storage_suite.py \
  data/samples/fs_e430_n4008_18.las output.oct /tmp/octree-storage-suite \
  --convert-binary target/perf/examples/real_dataset_bench \
  --read-binary target/perf/examples/storage_bench \
  --repetitions 3 --workers 1 --divisions 2 --points-per-block 10000 --max-decoded-mib 512 \
  --format-version v5 \
  --profile perf --cache-state 'sin purga; lecturas tras conversion y validacion'
```

El directorio debe ser nuevo. `--points-per-block` fija la misma partición para todas las
repeticiones (default 262144; el ejemplo usa 10000 para generar hijos en una muestra pequeña). Cada repetición produce su propia pareja y ejecuta los tres modos
con una apertura nueva. Se conservan comandos, códigos de salida, stdout/stderr, métricas crudas,
medianas/mínimos/máximos, hashes SHA-256 de fuente, binarios y salidas, Git/árbol sucio, perfil,
plataforma, arquitectura/CPU cuando disponible y versiones. El informe marca fallos y conserva lo ya ejecutado; no interpretar
resultados parciales como una suite completa. Los hashes se calculan fuera de los cronómetros.
La validación de conversión y el hashing calientan la caché: esta suite mide lecturas después de
conversión, nunca una lectura fría por defecto. El texto `--cache-state` registra lo ocurrido;
no purga automáticamente cachés.

Lecturas aisladas sobre una pareja existente:

```sh
target/perf/examples/storage_bench INPUT.oct hierarchy 5
target/perf/examples/storage_bench INPUT.oct reconstruct 5 512
target/perf/examples/storage_bench INPUT.oct progressive 5 512
```

Para una lectura fría hace falta un protocolo externo declarado de caché/sistema; comparar con
lecturas calientes por separado. Registrar CPU/RAM, disco/filesystem y carga del equipo junto al
informe cuando no puedan inferirse automáticamente. Para comparar implementaciones, usar la misma
fuente (SHA-256), opciones, contenido preservado, perfil y condición de caché, alternar v5/v6 y
conservar al menos tres ejecuciones exitosas por caso. Los hashes binarios deben corresponder a
las implementaciones ensayadas. No extrapolar estas muestras a 500 GB ni a navegador/FPS.

## Interpretación y límites

- `open_ms` mide sólo apertura primaria; `attach_ms`, asociación/validación del acompañante.
  `load_ms` mide lectura/decodificación/CRC: en reconstrucción es la carga residente; en progresión
  suma las lecturas de bloques. `verify_ms` separa el checksum de contenido XYZ/RGB/atributos.
  `full_ms` es reloj total desde apertura hasta verificación, con preflight, recorrido y overhead
  del benchmark; no es la suma de sólo las tres fases de I/O.
- Los percentiles son nearest rank por nodo, agrupados por nivel. Un nodo vacío puede tardar cero.
  `first_root_ms` suma lecturas del payload raíz, no mide primera imagen presentada. El checksum
  también incluye IDs/orden: debe mantenerse entre repeticiones del mismo modo, pero puede diferir
  entre reconstrucción por BlockId y recorrido BFS. No es un hash criptográfico.
- Se validan IDs/conteos y los CRC del reader. Se cargan payloads propios de todos los nodos,
  incluidos LOD: `points` puede superar el número de puntos de fuente. Reconstrucción conserva
  bloques y la jerarquía validada; no fusiona representantes LOD en una nube sin duplicación.
- V6 distingue `raw_payload_bytes` (streams transformados antes de LZ4) de
  `logical_payload_bytes` (XYZ canónico + RGB + columnas). El tamaño de pareja es la comparación neta.
- El preflight suma `point_count * size_of::<Point>() + raw_bytes` por bloque, contando otra vez
  la geometría como margen. Reconstrucción limita el total; progresión el máximo bloque vivo.
  Excluye índice residente, metadatos de contenedores, allocator, cola/recorrido y buffers
  transitorios: es presupuesto estimado de payload, **no un límite de RSS**. Para medir memoria
  usar además el profiler existente o `/usr/bin/time -v`.
- Los modos de payload requieren el acompañante cuando el formato lo exige; una pareja ausente,
  incorrecta o corrupta falla. `hierarchy` puede abrir sólo `.oct`. Usar `read-geometry` o
  `read-attribute` del benchmark anterior para la matriz selectiva; el nuevo recorrido usa bloques
  completos para establecer una referencia común.
- El cierre/sync se informa en conversiones nuevas con diagnósticos; reanudar desde un checkpoint
  puede saltarse fases. El runner no usa checkpoints. No sumar tiempos de workers solapados como
  tiempo de pared.

## Historial provisional: muestra multibloque (anterior al cierre de v5)

Esta serie usa la cabecera provisional `OCTVWFRM`. Se conserva como evidencia histórica,
no como baseline de v5 definitivo. Suite completa de tres repeticiones sobre `fs_e430_n4008_18.las` (111190 puntos de fuente),
con 10000 puntos/bloque, un worker, divisiones 2, perfil `perf` y caché sin purgar tras
conversión/validación/hashing. Equipo AMD Ryzen AI 9 365, Linux; árbol de trabajo con cambios.
[Informe JSON conservado](benchmarks/storage-v5-smoke-2026-10-13.json), con hashes exactos,
opciones y resultados crudos. Los logs y parejas de esta ejecución local están en
`/tmp/octree-storage-suite-multiblock-20261013`; esa ruta temporal no es un artefacto portable.

| Medida | Mediana | Mínimo–máximo |
| --- | ---: | ---: |
| Tamaño de pareja | 2256854 B | idéntico en las tres |
| Conversión completa | 69,872 ms | 65,246–79,444 ms |
| Escrituras ordenadas/liberación | 0,923 ms | 0,858–1,328 ms |
| Cierre y sync de candidatos | 1,068 ms | 1,049–1,604 ms |
| Apertura estructural (`hierarchy.open_ms`) | 0,233 ms | 0,213–0,349 ms |
| Carga residente (`reconstruct.load_ms`) | 17,854 ms | 16,118–19,227 ms |
| Reconstrucción total con verificación (`full_ms`) | 22,690 ms | 20,947–24,053 ms |
| Lecturas de hijos (`progressive.load_ms`) | 15,123 ms | 14,136–18,195 ms |

La jerarquía produjo 47 bloques en niveles 0–4, con 111768 puntos almacenados incluidos los LOD.
Checksums estables en cada modo en las tres repeticiones: `814062698caec8d6` (residente) y
`67cb12883d2e5d7c` (BFS). Con presupuesto de 1 MiB, residente rechaza antes de payload y BFS
completa; sin acompañante, la reconstrucción completa falla. Esto valida el instrumento y el
recorrido real de hijos, no el rendimiento a escala. Los tiempos de carga de cada modo tienen
condiciones de caché y retención distintas; no usar su diferencia como una mejora de algoritmo.

## Verificación v5 definitivo: muestra multibloque

Tres repeticiones sobre la misma fuente y opciones de la serie anterior, con `header_envelope=1`
y magic `OCTVFRM1`. [Informe JSON definitivo](benchmarks/storage-v5-final-smoke-2026-10-13.json)
con hashes de los binarios exactos, fuente, salidas y métricas crudas. Logs/parejas locales en
`/tmp/octree-storage-v5-final-suite-20261013`; la ruta temporal no es portable.
El tamaño conjunto fue 2256854 B en las tres ejecuciones, con 47 bloques en cinco niveles y
checksums estables en cada modo. No se compara rendimiento con el provisional: las series no
se alternaron ni controlaron toda la carga externa.

| Medida | Mediana | Mínimo–máximo |
| --- | ---: | ---: |
| Conversión completa | 69.469 ms | 68.479–73.525 ms |
| Escritura ordenada/liberación | 0.914 ms | 0.901–0.946 ms |
| Cierre/sync candidatos | 1.052 ms | 1.046–1.054 ms |
| Apertura estructural | 0.320 ms | 0.223–0.385 ms |
| Carga residente | 16.410 ms | 15.453–17.824 ms |
| Reconstrucción total con verificación | 21.279 ms | 20.244–22.839 ms |
| Lectura de hijos | 17.990 ms | 17.973–18.238 ms |

Playwright MCP comprobó la muestra web final y una conversión OPFS real de 111190 puntos
([informe de navegador](benchmarks/storage-v5-final-browser-2026-10-13.json)):
ambos archivos emitieron `OCTVFRM1`, y la pareja se reabrió mediante Blob como nueve nodos y ocho
columnas. La muestra de 4096 puntos abre sin atributos y con su acompañante; se rechazan el
provisional, `v6.2-pre` y un acompañante incorrecto. Probe reproducible:
`examples/web-host/tests/storage-format.browser.mjs`, ejecutar como module Worker y esperar
`type === "probeResult"` (ignorar los mensajes de progreso). No requiere GPU.
El navegador MCP no ofreció adaptador WebGPU; esta evidencia no certifica presentación ni LOD GPU.
La consola sólo registró el 404 de `favicon.ico`, no errores de los contratos comprobados.

## Primera comparación v5/v6.1-pre — 13-10-2026

[Informe comparativo](benchmarks/storage-v6-pre1-comparison-2026-10-13.json): tres procesos por
formato y fuente, alternando v5→v6 / v6→v5 / v5→v6. Misma fuente, binarios multiperfil, opciones,
perfil `perf`, un worker, divisiones 2 y 10000 puntos/bloque. Salidas en **tmpfs `/tmp`**, caché sin
purga tras conversión/validación/hashing; no representa escritura a un disco ni lecturas frías.
Serie publicada después de compilar/verificar, sin compilación ni conversión web simultáneas;
la carga externa y frecuencia CPU no están aisladas. CPU AMD Ryzen AI 9 365. El informe conserva
comandos, SHA-256 de fuente/binarios/reportes, muestras y estadísticas. Logs/parejas temporales
están bajo `source_report_root`; esa ruta no es portable.

| Fuente | Pareja v5 | Pareja v6.1-pre | Reducción neta | Conversión mediana v5 / v6 |
| --- | ---: | ---: | ---: | ---: |
| Outer Banks e430 | 2256854 B | 2022428 B | 10.39 % | 75.037 / 75.591 ms |
| Outer Banks e432 | 3054522 B | 2754971 B | 9.81 % | 87.787 / 102.788 ms |
| Bunny parcial | 414107 B | 211185 B | 49.00 % | 18.652 / 18.968 ms |

| Fuente | Carga residente v5 / v6 | Lectura de hijos v5 / v6 | Pico RSS residente v5 / v6 |
| --- | ---: | ---: | ---: |
| Outer Banks e430 | 17.653 / 16.450 ms | 17.036 / 17.191 ms | 6627328 / 6868992 B |
| Outer Banks e432 | 16.632 / 21.351 ms | 22.627 / 20.675 ms | 8126464 / 8028160 B |
| Bunny parcial | 5.454 / 4.809 ms | 5.213 / 4.807 ms | 3829760 / 3567616 B |

Los checksums lógicos coinciden en todas las repeticiones y entre formatos para cada modo; los
archivos v5 regenerados también coinciden byte a byte con las dos parejas versionadas anteriores.
Los deltas reducen tamaño en estas muestras, pero no hay una mejora general de velocidad ni de
RSS demostrada. RSS se obtuvo con `/usr/bin/time -v` en procesos adicionales de reconstrucción
residente; no es pico de conversión, GPU ni WASM. El Bunny local no versionado no se redistribuye.

Reproducción por formato, usando el mismo runner y alternando invocaciones de una repetición:

```sh
python3 crates/viewer_convert/benchmarks/storage_suite.py \
  data/samples/fs_e430_n4008_18.las output.oct /tmp/storage-v6-pre1-run1 \
  --convert-binary target/perf/examples/real_dataset_bench \
  --read-binary target/perf/examples/storage_bench --repetitions 1 \
  --workers 1 --divisions 2 --points-per-block 10000 --max-decoded-mib 512 \
  --format-version v6.1-pre --profile perf --cache-state 'sin purga; tras conversion/validacion/hashing'
# Repetir con --format-version v5 y directorios nuevos, alternando el orden en tres rondas.
```

Se corrigió durante esta entrega una reserva de `Point` que trataba bytes como número de elementos;
ambos formatos del ensayo usan el lector corregido. No mezclar estos tiempos con los anteriores
como si sólo hubiera cambiado el formato. Los límites de v6 se prueban tanto sobre bytes lógicos
como sobre streams transformados; un delta pequeño no permite saltarse la expansión lógica.

[Playwright MCP / OPFS](benchmarks/storage-v6-pre1-browser-2026-10-13.json) convirtió 111190 puntos
a v6.1-pre (mayor 6/revisión 1 en ambos archivos), abrió Blob con/sin acompañante y rechazó
provisional, v6.2-pre y acompañante incorrecto. Se descargaron las parejas web v5/v6 y el lector
nativo reconstruyó 37 bloques / 111993 puntos incluidos LOD, con checksum idéntico
`9f17732d9dcbe040`. La conversión web predeterminada sigue siendo v5. Se refrescaron scripts de
worker para evitar caché de módulos de desarrollo. MCP no ofreció adaptador WebGPU: quedan
pendientes refinamiento/render/picking con v6 en un navegador con GPU operativa.

## Cierre pendiente para v6

Extender la revisión implementada, ejecutar esta matriz en el corpus acordado
(Belvedere, Majadas y Albuera además de muestras pequeñas) y comparar calidad/atributos junto a
bytes y tiempos. La muestra pequeña verifica el instrumento; no demuestra rendimiento a escala.
La carga progresiva de render requiere además sus pruebas de caché, presupuesto, frontera LOD y
navegador. Ver [contratos e identificación de versiones](STORAGE_V6_PREPARATION.md).

## Piloto completo de Belvedere — 13-10-2026

[Informe y muestras crudas](benchmarks/storage-v6-pre1-belvedere-pilot-2026-10-13.json). Se usó
Belvedere (81.682.329 puntos fuente, 1.343 bloques, 81.706.211 filas con LOD), un worker,
divisiones 2 y 262.144 puntos/bloque. Una conversión v6.1-pre y una conversión v5 definitiva
corrieron en serie desde el mismo ejecutable `real_dataset_bench`; el orden no se alternó y la
conversión v6 ocurrió primero. Las salidas se validaron durante conversión. Después se hicieron
tres recorridos BFS por pareja con el mismo `storage_bench`, en el filesystem local del workspace
con page cache caliente. El hash de la fuente queda registrado en el informe; no se redistribuye el LAZ.

| Medida | v5 | v6.1-pre |
| --- | ---: | ---: |
| Tamaño `.oct` | 981.063.326 B | 980.976.976 B |
| Tamaño `.octattrs` | 610.586.859 B | 610.586.859 B |
| Pareja | 1.591.650.185 B | 1.591.563.835 B |
| Conversión observada (una ejecución) | 46.292 ms | 46.617 ms |
| RSS máximo muestreado de conversión | 100.929.536 B | 104.083.456 B |
| Pico temporal lógico muestreado | 4.633.296.377 B | 4.642.782.102 B |
| Reconstrucción residente `load_ms` (una ejecución) | 7.459 ms | 7.542 ms |
| RSS máximo de reconstrucción, `/usr/bin/time -v` | 3.364.032 KiB | 3.363.692 KiB |
| Lectura BFS `load_ms`, mediana de 3 | 6.706 ms | 6.719 ms |
| RSS máximo del proceso de lectura | 18.148 KiB | 17.864 KiB |

El ahorro neto fue **86.350 B (0,0054 %)** y está sólo en geometría. Los checksums fueron
`0737ca650c6081b4` en las seis pasadas; cada una leyó 1.343 bloques y 81.706.211 puntos. `load_ms`
incluye lectura, decodificación y CRC, pero estos archivos caben en caché: no estima disco frío,
carga por cámara, reconstrucción residente, FPS, GPU ni transiciones visuales. Los tiempos de
conversión son una observación por versión, con orden fijo; no sustentan una diferencia de velocidad.
Las reconstrucciones residentes produjeron en ambos formatos 81.706.211 puntos y checksum
`36909ff142199a4e`; el RSS externo fue 3.364.032 KiB (3,21 GiB) y 3.363.692 KiB (3,21 GiB),
respectivamente. Es una sola observación por versión y no sustituye la matriz repetida ni mide
memoria del renderer.
El RSS de conversión es un máximo muestreado cada 100 ms y el temporal es tamaño lógico observado;
ambos pueden perder picos breves.

La pareja Belvedere preexistente fue rechazada por el lector (`ProvisionalFormat`), pese a tener
tamaños históricos registrados. Para evitar mezclar cabeceras, el baseline se regeneró desde LAZ
con v5 definitivo; el hash de fuente y hashes de ambos resultados están en el JSON. Esta evidencia
resuelve un hueco de compatibilidad del corpus local, pero deja pendiente la serie alternada formal
de tres conversiones por versión y las conversiones v6 de Majadas y Albuera.

El perfilador `profile_conversion.py` acepta ahora `--format-version` y `--points-per-block` y
registra el hash de fuente y opciones efectivas en su informe. Reproducir una conversión aislada:

```sh
python3 crates/viewer_convert/benchmarks/profile_conversion.py \
  data/datasets/belv_2009_digital-aerial_pcd_copc.laz target/v6-pre-belvedere/run-1 \
  --workers 1 --divisions 2 --interval-ms 100 --points-per-block 262144 \
  --format-version v6.1-pre
```
