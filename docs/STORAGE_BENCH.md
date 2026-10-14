# Benchmarks formales de almacenamiento

Protocolo añadido el 13-10-2026 para preparar comparaciones v5/v6. Están soportados v5 definitivo y los experimentales v6.1-pre/v6.2-pre (`OCTVFRM1`, envelope 1); las series provisionales
se conservan separadas y no certifican el formato final.
Última serie: [comparación repetida del 14-10-2026](benchmarks/storage-v5-v6-pre1-comparison-2026-10-14.json),
con Belvedere completo y dos muestras multibloque en NVMe local.
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

Extender la revisión implementada y la matriz repetida a Majadas y Albuera; Belvedere y dos
muestras pequeñas ya tienen tres conversiones por formato en la serie del 14-10-2026. Comparar
calidad/atributos junto a bytes y tiempos, y completar memoria. La muestra pequeña verifica el
instrumento; no demuestra rendimiento a escala.
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
resuelve un hueco de compatibilidad del corpus local. La serie alternada formal de tres conversiones
por versión se completó el 14-10-2026 (véase abajo); Majadas y Albuera siguen pendientes.

El perfilador `profile_conversion.py` acepta ahora `--format-version` y `--points-per-block` y
registra el hash de fuente y opciones efectivas en su informe. Reproducir una conversión aislada:

```sh
python3 crates/viewer_convert/benchmarks/profile_conversion.py \
  data/datasets/belv_2009_digital-aerial_pcd_copc.laz target/v6-pre-belvedere/run-1 \
  --workers 1 --divisions 2 --interval-ms 100 --points-per-block 262144 \
  --format-version v6.1-pre
```


## Comparación repetida v5/v6.1-pre — 14-10-2026

[Informe completo con muestras crudas](benchmarks/storage-v5-v6-pre1-comparison-2026-10-14.json). 18
conversiones y 54 procesos de lectura completados, tres conversiones por formato y fuente. Orden por
ronda: v5→v6, v6→v5, v5→v6; operaciones en serie, sin compilación simultánea. Perfil `perf`, un
worker, subdivisión 2; 10000 puntos/bloque en Outer Banks y 262144 en Belvedere. Presupuesto
estimado de reconstrucción: 512 MiB y 8192 MiB, respectivamente.

Equipo: AMD Ryzen AI 9 365, 30 GiB de RAM, Linux x86_64. Las salidas están en el NVMe local
`/dev/nvme0n1p3`, Btrfs con `compress=zstd:1`, no en tmpfs. Tamaños = bytes lógicos completos de
archivo, incluyendo índices/cabeceras, no bloques físicos ocupados tras compresión Btrfs. Caché sin
purgar; lecturas posteriores a conversión, validación y hashing. No son lecturas frías ni latencia
pura del disco. Frecuencia CPU y carga externa sin aislar; el JSON registra carga media
antes/después de cada suite, fuente/binarios/salidas SHA-256, Git, comandos y todas las muestras.

El código ensayado es `1375474`; un único par de binarios selecciona ambas versiones. La preview
serializable es **v6.1-pre** (delta-varint con fallback directo). El prototipo
`OccupiedOctree`/`OccupiedPointBlock` no se serializa y no interviene en estos tamaños/tiempos.

| Fuente | Puntos fuente | Bloques | Pareja v5 (B) | Pareja v6.1-pre (B) | Ahorro neto |
| --- | ---: | ---: | ---: | ---: | ---: |
| Outer Banks e430 | 111190 | 47 | 2256854 | 2022428 | 10.3873 % |
| Outer Banks e432 | 149730 | 55 | 3054522 | 2754971 | 9.8068 % |
| Belvedere | 81682329 | 1343 | 1591650185 | 1591563835 | 0.0054 % |

### Outer Banks e430: tiempos en ms

Cada celda informa **mediana (mínimo–máximo)** de tres procesos.

| Medida | v5 | v6.1-pre |
| --- | ---: | ---: |
| Conversión completa | 84.898 (84.017–107.516) | 88.010 (87.316–114.594) |
| Preparación de bloques/compresión | 21.788 (21.612–31.650) | 25.877 (18.918–37.974) |
| Escritura ordenada/liberación | 1.807 (1.393–2.157) | 1.581 (1.270–2.152) |
| Cierre/sync candidatos | 11.114 (10.190–13.955) | 11.049 (10.287–13.167) |
| Apertura estructural | 0.320 (0.227–0.406) | 0.317 (0.317–0.323) |
| Carga residente completa | 14.073 (13.542–14.737) | 18.404 (15.259–22.102) |
| Reconstrucción total con verificación | 18.191 (17.541–18.768) | 23.207 (18.741–29.272) |
| Lectura progresiva BFS | 14.066 (10.041–20.243) | 20.996 (15.399–21.260) |

### Outer Banks e432: tiempos en ms

Cada celda informa **mediana (mínimo–máximo)** de tres procesos.

| Medida | v5 | v6.1-pre |
| --- | ---: | ---: |
| Conversión completa | 105.287 (84.519–113.240) | 99.908 (90.013–151.727) |
| Preparación de bloques/compresión | 31.200 (21.980–32.158) | 27.443 (25.669–51.055) |
| Escritura ordenada/liberación | 1.904 (1.624–2.272) | 1.770 (1.650–2.688) |
| Cierre/sync candidatos | 13.044 (9.255–14.608) | 10.377 (9.057–17.980) |
| Apertura estructural | 0.311 (0.273–0.364) | 0.332 (0.272–0.404) |
| Carga residente completa | 15.868 (15.434–28.360) | 21.764 (20.067–23.868) |
| Reconstrucción total con verificación | 20.407 (20.225–37.678) | 28.525 (24.937–28.545) |
| Lectura progresiva BFS | 18.987 (16.476–19.882) | 16.825 (16.805–26.251) |

### Belvedere: tiempos en ms

Cada celda informa **mediana (mínimo–máximo)** de tres procesos.

| Medida | v5 | v6.1-pre |
| --- | ---: | ---: |
| Conversión completa | 46505.074 (44686.522–46935.079) | 48045.772 (47792.092–49200.170) |
| Preparación de bloques/compresión | 10487.221 (10486.670–10542.778) | 11635.899 (11634.688–11713.729) |
| Escritura ordenada/liberación | 347.862 (327.527–373.623) | 420.305 (406.016–423.396) |
| Cierre/sync candidatos | 616.419 (579.524–1049.614) | 698.219 (682.045–732.344) |
| Apertura estructural | 4.589 (4.447–7.939) | 4.876 (4.362–7.864) |
| Carga residente completa | 7412.501 (7380.167–7421.199) | 7387.826 (7348.492–7413.250) |
| Reconstrucción total con verificación | 10089.966 (10057.764–10098.777) | 10091.456 (10021.586–10101.461) |
| Lectura progresiva BFS | 6709.543 (6670.204–6738.910) | 6678.187 (6648.610–6711.281) |

Los conteos y checksums lógicos coinciden entre formatos en las seis pasadas de cada modo por
fuente; además cada formato produce salidas byte a byte deterministas entre sus tres conversiones.
Reconstrucción residente y BFS usan orden distinto y sus checksums se comparan por modo. Belvedere
reconstruye 81706211 filas incluidos LOD: `36909ff142199a4e` residente y `0737ca650c6081b4` BFS.

Interpretación: el ahorro de tamaño vuelve a ser 10,39 %/9,81 % en las muestras Outer Banks, pero
sólo **86350 B (0,0054 %)** en Belvedere. Esta serie cierra la repetición alternada de Belvedere; no
demuestra una mejora general de velocidad. Los rangos se conservan para no confundir dispersión con
diferencias de rendimiento. La escritura ordenada incluye liberación de spools, y el cierre incluye
`sync_all`; ninguna de ambas métricas incluye toda la conversión. No sumar sus medianas como si
fueran la mediana de un tiempo combinado.

No se midió RSS en esta serie ni hay navegador/GPU/red; las cifras anteriores de memoria siguen
siendo evidencia separada. Majadas y Albuera permanecen pendientes para v6. Los archivos y logs
locales están bajo `source_report_root` en el JSON; no se redistribuyen los datasets.

Reproducción: usar el runner `storage_suite.py` descrito arriba con una repetición por invocación y
directorios nuevos, alternar los formatos en tres rondas y conservar las mismas opciones. El informe
contiene los 18 comandos exactos y cada reporte original completo.

## Representación física v6.2-pre — 14-10-2026

Objetivo: menos bytes e I/O, aceptando CPU para reconstrucción. La representación en memoria
del prototipo anterior no decide el formato. [Contrato binario](../crates/viewer_format/FORMAT_V6_PRE2.md):
arrays por profundidad dentro de cada bloque direccionable, registros de seis bytes
(máscara u8, conteo/octante u8, índice u32), filas en varints y residuales empaquetados.
El índice exterior de LOD/atributos se conserva; no es todavía un único octree global por nube.

### Byte de conteo: inferir o almacenar octante

[Streams materializados y muestras](benchmarks/storage-v6-pre2-octants-2026-10-14.json).
Prefijos en orden fuente de 32768 puntos de cuatro LAS/LAZ, sin pasar por f32. Tres procesos
por fuente, cinco iteraciones por proceso, dos anchos residuales y dos variantes de octante:
15 muestras por combinación y 240 decodificaciones exactas. Orden invertido en iteraciones
pares. Compilación/tests y navegador terminaron antes de medir. Perfil perf, Ryzen AI 9 365.

Los cuatro bits bajos representan 0–8 hijos. Los tres siguientes pueden almacenar el octante
propio y el último queda reservado a cero. Ambas variantes tienen la misma longitud antes
de LZ4. El octante explícito puede reducir CPU en algunos casos, pero añade redundancia y
cambia la compresibilidad. En los ocho casos de esta serie **inferir siempre comprimió mejor**.
Por prioridad de I/O se favorece inferencia; el escritor evalúa también la variante explícita
y sólo la elige si sus bytes finales son estrictamente menores. No se garantiza acelerar CPU.

| Fuente | Bits residuales | Bytes inferidos | Bytes con octante | Decode inferido / explícito (ms) |
| --- | ---: | ---: | ---: | ---: |
| Outer Banks e430 | 24 | 516668 | 602299 | 1.400 / 1.906 |
| Outer Banks e430 | 28 | 368434 | 368980 | 0.484 / 0.414 |
| Belvedere | 24 | 352376 | 355507 | 0.454 / 0.456 |
| Belvedere | 28 | 369678 | 369754 | 0.352 / 0.346 |
| Majadas | 24 | 301631 | 301752 | 0.648 / 0.561 |
| Majadas | 28 | 345342 | 345357 | 0.670 / 0.582 |
| Albuera | 24 | 521939 | 605663 | 2.214 / 2.934 |
| Albuera | 28 | 385764 | 386715 | 0.817 / 0.724 |

Tiempos = medianas; rangos y muestras están en el JSON. `decode_ms` incluye validación y
reconstrucción del stream **ya descomprimido**, no I/O, LZ4 ni proyección f32. `encode_ms`
incluye serializar y comprimir, pero no construir el candidato común. Estos bytes son sólo
geometría materializada; no compararlos con parejas completas sin sumar metadata/atributos.

### Parejas completas y lecturas

[Informe de archivos](benchmarks/storage-v6-pre2-comparison-2026-10-14.json).
Perfil perf, un worker, división 2; 10000 puntos/bloque en las muestras y 262144 en Belvedere.
NVMe local `/dev/nvme0n1p3`, Btrfs `compress=zstd:1`; bytes lógicos de archivo, no ocupación
física Btrfs. Lecturas calientes tras conversión/validación/hashing. Tamaños incluyen `.oct`,
`.octattrs`, índices y CRC. Los tres casos pequeños tienen tres conversiones por formato en
orden cíclico v5/pre1/pre2. Belvedere es **un piloto con una conversión por formato**, orden
fijo v5→pre1→pre2, una reconstrucción residente y tres lecturas BFS por pareja. No confundir
estos tiempos de conversión con una comparación formal alternada de tres conversiones.

| Fuente | Pareja v5 (B) | Pareja pre1 (B) | Pareja pre2 (B) | Bloques ocupación pre2 |
| --- | ---: | ---: | ---: | ---: |
| Outer Banks e430 | 2256854 | 2022428 | 2022428 | 0 |
| Bunny parcial | 414107 | 211185 | 211185 | 0 |
| Sintetica agrupada | 91115 | 91116 | 84537 | 1 |
| Belvedere | 1591650185 | 1591563835 | 1590761099 | 80 |

| Fuente | Conversión v5 / pre1 / pre2 (ms) | Carga residente (ms) | BFS (ms) |
| --- | ---: | ---: | ---: |
| Outer Banks e430 | 72.645 / 89.574 / 124.072 | 12.504 / 17.883 / 13.922 | 9.483 / 16.927 / 12.032 |
| Bunny parcial | 27.059 / 34.886 / 33.848 | 5.419 / 5.169 / 5.126 | 4.944 / 4.818 / 3.563 |
| Sintetica agrupada | 12.844 / 13.876 / 13.075 | 0.801 / 0.805 / 0.768 | 0.805 / 0.803 / 0.768 |
| Belvedere | 43835.841 / 48494.321 / 59758.121 | 7471.094 / 7644.937 / 7407.468 | 6816.276 / 6670.188 / 6564.843 |

En las muestras, tiempos = medianas de tres procesos. En Belvedere, conversión/residente
son observaciones únicas y BFS es mediana de tres. Preparación, escritura ordenada y sync
están separados en las métricas crudas. Coinciden conteos y checksums entre los tres formatos
en cada modo; las salidas por formato son deterministas en los casos repetidos.

El caso sintético agrupado conserva todos los puntos/atributos y reduce la pareja frente a
pre1 en 6579 B (7,22 %). Outer Banks y Bunny eligen fallback y mantienen el tamaño pre1.
Belvedere elige ocupación en 80 de 1343 bloques y ahorra **802736 B frente a pre1 (0,0504 %)**
o 889086 B frente a v5 (0,0559 %). Es una mejora física real pero pequeña en ese dataset;
no implica una reducción grande general. En este piloto convertir pre2 tarda más, y las
lecturas no permiten afirmar una mejora general de velocidad.

### Navegador y pendientes

[Playwright MCP / OPFS](benchmarks/storage-v6-pre2-browser-2026-10-14.json) convirtió una LAS
sintética agrupada a v5/pre2, abrió Blob con/sin acompañante y rechazó provisional, pre3 y
acompañante incorrecto. Ocho bloques de la salida web pre2 usan ocupación. Al reconstruir
ambas parejas con el lector nativo resultaron 4160 filas incluidos LOD y checksum residente
`70f71011fabdb3bf`. El único error de consola fue favicon.ico 404; no hubo adaptador WebGPU.
Esto no certifica presentación, FPS o refinamiento GPU.

Pendiente: repetir conversiones pre2 alternadas en Belvedere y extender archivos completos
a Majadas/Albuera; estudiar más perfiles/orden de streams y compactación del índice exterior.
Un único array global por profundidad necesita su propio contrato de grilla/raíces/índices y
lecturas paginadas. Medir esa representación física completa, preservando precisión y filas.
Las optimizaciones de memoria/CPU pueden hacerse tras los traits sin redefinir estos bytes.

Reproducir con el runner existente y `--format-version v6.2-pre`; conservar las mismas fuentes,
opciones y condiciones para v5/pre1. El JSON guarda comandos, hashes binarios/fuentes/salidas,
código medido y resultados crudos. La fuente sintética se genera con el script conservado en
el informe de navegador; las demás fuentes se usan localmente sin redistribuirlas.

## Diccionarios de atributos — 14-10-2026

El [experimento independiente](ATTRIBUTE_DICTIONARY_BENCH.md) compara streams materializados
raw/LZ4, diccionario por columna adaptativo y combinaciones completas de atributos + RGB. En
Belvedere completo los payloads vigentes ocupan 610.586.795 B; los streams experimentales con
cabeceras ocupan 439.593.178 / 341.724.528 B. No son archivos `.octattrs` compatibles. El
diccionario conjunto aumenta tamaño en Outer Banks y la sintética y acopla consultas selectivas.
El informe conserva CPU, cardinalidad, hashes, limitaciones y siguientes candidatos físicos.
