# Atributos físicos v6.3-pre — 14-10-2026

Esta entrega integra codecs **constante y diccionario por columna/bloque** en archivos válidos,
con fallback raw/LZ4 y la geometría de pre2. Es optativa; v5 sigue predeterminado. La
[especificación](../crates/viewer_format/FORMAT_V6_PRE3.md) define identidad, bytes, límites y
compatibilidad. El [experimento anterior](ATTRIBUTE_DICTIONARY_BENCH.md) era un contenedor
independiente de 32 B por stream; sus tamaños/tiempos no equivalen a esta integración.

## Protocolo

Se convierten Outer Banks e430, Bunny parcial, sintética agrupada y Belvedere completo. Tres
rondas seriales por muestra, alternando orden pre2→pre3, pre3→pre2, pre2→pre3; un proceso por
conversión, seguido por una apertura, una reconstrucción residente y una lectura BFS progresiva.
Se verifican hashes estables entre conversiones de la misma revisión y checksums lógicos
idénticos entre revisiones para cada modo. Incluyen todos los atributos, RGB y filas LOD.

Cargo `perf`, Ryzen AI 9 365, Linux x86_64, NVMe Btrfs zstd:1. Sin purgar caché; lecturas tras
conversión/validación/hashing. Tamaños lógicos de archivos, no asignación física después de la
compresión transparente del filesystem. Un trabajador, división 2, 10.000 puntos/bloque en las
tres muestras pequeñas y 262.144 en Belvedere. Presupuesto de lectura 512/8.192 MiB respectivamente.
El protocolo y cada comando/hash/métrica quedan en el JSON enlazado al cerrar la medición.

La codificación construye candidatos y selecciona el menor contando sus bytes reales. Esta
revisión elimina la cabecera constante y reduce a 10 B la del diccionario; no tiene las
cabeceras experimentales de 32 B. La lectura conserva independencia: solicitar clasificación
no carga RGB ni tiempo GPS. La geometría y el orden de filas permanecen iguales a pre2.

## Pendientes posteriores a esta entrega

- Medir Majadas/Albuera y pico conjunto de temporales/caché; los límites de buffers no son RSS.
- Probar modo de enteros por rango/bitpacking y selección de candidatos más barata si se justifica.
- Comparar paleta RGB8+RGB16 conjunta o derivación exacta del RGB de presentación, conservando
  atributos numéricos separados. No deducir su ahorro del diccionario conjunto del experimento.
- Ensayar prefijos `first_child` reconstruidos y hojas implícitas en `.oct`, por separado/juntos,
  contando todo el stream tras LZ4 y CPU. Esta entrega no cambia esa topología.

## Resultados: archivos válidos completos

[Reporte íntegro](benchmarks/storage-v6-pre3-comparison-2026-10-14.json): 24 conversiones y 72
procesos de lectura, con comandos, hashes, métricas por ejecución y checksums. Todas las
conversiones de una misma muestra/revisión producen hashes idénticos. Los checksums de cada
modo coinciden entre pre2 y pre3.

| Muestra | Pareja pre2 (B) | Pareja pre3 (B) | Reducción | Conversión mediana pre2/pre3 (ms) |
| --- | ---: | ---: | ---: | ---: |
| Outer Banks e430 | 2.022.428 | 1.905.318 | 5,79 % | 108,372 / 114,724 |
| Bunny parcial | 211.185 | 207.320 | 1,83 % | 32,100 / 37,480 |
| Sintética agrupada | 84.537 | 84.319 | 0,26 % | 14,386 / 15,998 |
| Belvedere completo | 1.590.761.099 | 1.419.356.066 | **10,78 %** | 62.368,255 / 68.922,918 |

Belvedere conserva 81.682.329 puntos fuente y 81.706.211 filas con LOD en 1.343 bloques. El
`.oct` ocupa **980.174.240 B** en ambas revisiones; sus campos de directorio/CRC cambian, pero
su geometría y longitud no. `.octattrs` baja de **610.586.859 a 439.181.826 B**, un **28,07 %**,
contando su envoltura completa. Se usan **10.744 payloads constantes y 1.833 diccionarios**;
la geometría sigue eligiendo ocupación en 80 bloques. No es una proyección ni el tamaño de un
contenedor experimental.

| Belvedere, operación | Pre2 mediana [mín., máx.] | Pre3 mediana [mín., máx.] |
| --- | ---: | ---: |
| Conversión completa | 62,368 [58,576, 65,363] s | 68,923 [68,851, 72,081] s |
| Reconstrucción residente (`load_ms`) | 7,640 [7,473, 7,889] s | 9,420 [9,410, 9,438] s |
| BFS, un bloque vivo (`load_ms`) | 7,996 [6,999, 8,009] s | 8,691 [8,628, 9,486] s |

La conversión tarda un 10,51 % más; la lectura/decodificación caliente, un 23,29 % / 8,70 % más
para residente/BFS. El ahorro de I/O no demuestra una mejora de latencia en este entorno: los
archivos están en caché y la reconstrucción consume CPU. La conversión es tiempo de pared
end-to-end, no tiempo CPU aislado. `load_ms` excluye la verificación posterior del checksum;
los tiempos completos y métricas de cada proceso están en el JSON. Tres repeticiones no
permiten generalizar a almacenamiento remoto, caché fría, otros corpus o hardware.

Checksums Belvedere: residente `36909ff142199a4e`, BFS `0737ca650c6081b4`, iguales en las seis
conversiones. El runner sólo informa una estimación controlada de memoria, no RSS; no se ha
medido el pico de las nuevas tablas/hash maps ni el conjunto de caché/render.

## WASM y navegador

[Registro reproducible](benchmarks/storage-v6-pre3-browser-2026-10-14.json): Playwright MCP con
HeadlessChrome 155, conversión real OPFS de una LAS sintética de paleta repetida (4.096 puntos),
apertura Blob con/sin acompañante, y rechazo de provisional, pre4 y acompañante incorrecto.
Se reconstruyen completamente las parejas escritas por WASM usando el lector nativo:

- Pre2: `.oct` 55.715 B + `.octattrs` 25.441 B.
- Pre3: `.oct` 55.715 B + `.octattrs` 6.486 B; 96 constantes y 24 diccionarios.
- Ambas: 4.160 filas con LOD, 16 bloques, checksum `23f8096fa9f433c5`.

La prueba ejercita el **encoder WASM**, la metadata del lector Blob y la interoperabilidad de
payloads con el decoder nativo. No acredita decodificación/render/presentación GPU en navegador.
No hubo errores del conversor/probe; la página de listado local produjo únicamente un 404 de
`favicon.ico`. Se regeneraron bindings WASM y paquete local; no se modificó la interfaz.

## Verificación y dependencias

Pasan 320 tests nativos del workspace (3 GPU ignorados), 93 tests de `viewer_format --all-targets`,
Clippy del workspace y del target WASM con `-D warnings`, formato, 19 tests Python del runner,
28 tests Node del host, enlaces locales y `git diff --check`. Los tests nuevos ejercitan codecs,
componentes/tipos, NaNs/ceros con signo, CRC/corrupción/límites, gates de revisión y conteos exactos
de bytes leídos por columna. LAS/LAZ, EPT y checkpoints incluyen pre3 en su matriz de versiones.

Durante el trabajo cambiaron `Cargo.lock`, el manifiesto raíz y el de `viewer_render_wgpu`
fuera de esta entrega (incluida la migración wgpu 24→30 / glam 0,29→0,34). El nuevo lock pide
dependencias no disponibles offline. Se preservan esos cambios y se excluyen del commit. El cierre nativo/WASM
se repitió en un worktree aislado con **idénticas fuentes de la feature** y los manifiestos/lock de HEAD usados
para compilar los binarios del benchmark. El JSON registra los hashes de ambos estados. Esta evidencia
corresponde a esas dependencias; la actualización ajena de dependencias no queda validada aquí.

## Uso

```sh
cargo run --release -p viewer_convert -- INPUT.laz OUTPUT.oct --format-version v6.3-pre
```

Se puede repetir cada comando exacto del reporte, con directorios nuevos. No mezclar sidecars
entre revisiones ni interpretar pre3 como v6 final. V5/pre1/pre2 siguen leyendo y escribiendo sus
layouts anteriores. Los siguientes experimentos pendientes están al inicio y en el README.
