# R3: exploración CPU de anchura de región — 05-11-2026

**Observado:** en los 17 nodos elegidos, la variante regional con acceso directo a una región presenta cocientes frente a `fast48` distintos entre internos y hojas. No se elige una anchura universal ni se cambia una decisión del proyecto. Es un laboratorio CPU por nodo y profundidad, no una medida del renderizado ni de un dataset completo.

## Método y límites

Se prueban anchuras de 8, 10, 12, 15, 16, 18 y 21 bits por eje: se separan el prefijo de región y la clave Morton local; la clave local es `u32` para 8/10 y `u64` para 12–21. El directorio usa `BTreeMap<[u64; 3], Vec<_>>`. `fast=false` siempre utiliza el mapa; `fast=true` usa un vector directo si los rangos del nodo caben en una región, y el mapa en otro caso. No implica que todos los nodos con `fast=true` entren en la ruta directa.

Se miden preparación, ordenación y consumo; hay 3 calentamientos y 9 muestras, con rotación del orden de 16 candidatos por ronda. La auditoría de capacidades y regiones es una invocación separada, sin tiempo incluido. La salida de cada candidato se compara bit a bit con las celdas de producción para la profundidad seleccionada; la ejecución terminó con código 0 para los 17 nodos. `fast48` es la referencia principal y usa la representación de 48 bytes solo cuando el rango cabe en 21 bits por eje; si no, recurre a la representación de 80 bytes. `legacy80` es otra referencia, distinta de `fast48` y del experimento anterior.

La selección toma los tres primeros nodos internos y tres hojas por dataset con 100–100.000 puntos, y añade los IDs especiales presentes. Puntos efectivos y clase:

| Dataset | Internos: ID (puntos) | Hojas: ID (puntos) |
|---|---|---|
| Majadas | 0 (8504), 1 (23708), 6 (4745) | 2 (782), 3 (464), 5 (30996), 120259084303 (8131), 300647710726 (132) |
| Belvedere | 0 (11549), 1 (4521), 2 (266) | 10 (1401), 26 (5454), 35 (6798), 120259084303 (1), 124554051592 (2), 300647710726 (1) |

Los tres últimos IDs de Belvedere tienen rangos de 22 bits y fuerzan el fallback de `fast48`. Sus tiempos muy pequeños son especialmente sensibles al ruido. El número de regiones observadas llega a 158 según nodo y anchura. La cota inferior del directorio cuenta, por región, el prefijo de 24 bytes y el encabezado `Vec` de 24 bytes; es cero en la ruta directa. No incluye nodos ni capacidad interna del árbol, asignador ni RSS; `btree_actual_bytes` es desconocido. El CSV recoge regiones, capacidad de entradas y cota por combinación.

## Resultados

Cada cociente de muestra divide el tiempo total de una variante por el de `fast48` con el mismo número de ronda; los candidatos se ejecutan en posiciones distintas. La tabla resume, **dentro de cada dataset y clase**, la mediana de las medianas de cocientes por nodo, seguida del intervalo entre esas medianas. Son muestras emparejadas por ronda, no una inferencia estadística ni una comparación entre datasets. Menor que 1 indica menor tiempo en esta captura. Se muestra `fast=true`; `fast=false` y `legacy80` están en el CSV.

| Anchura | Majadas internos (3) | Majadas hojas (5) | Belvedere internos (3) | Belvedere hojas (6) |
|---|---:|---:|---:|---:|
| 8 | 0,933 [0,895–0,943] | 2,815 [1,281–3,506] | 0,931 [0,906–1,022] | 2,799 [1,574–3,846] |
| 10 | 0,897 [0,842–0,934] | 2,637 [1,280–3,486] | 0,933 [0,880–1,022] | 2,814 [1,616–3,804] |
| 12 | 0,916 [0,882–0,918] | 2,643 [1,251–3,428] | 0,897 [0,872–1,023] | 2,373 [1,634–3,756] |
| 15 | 0,870 [0,849–0,908] | 2,584 [1,248–2,817] | 0,891 [0,846–1,018] | 2,333 [1,639–2,807] |
| 16 | 0,866 [0,814–0,909] | 2,588 [1,247–2,808] | 0,894 [0,839–1,020] | 2,347 [1,634–2,777] |
| 18 | 0,865 [0,788–0,904] | 2,577 [1,246–2,814] | 0,896 [0,834–1,020] | 2,349 [1,621–2,799] |
| 21 | 0,864 [0,775–0,903] | 2,574 [1,250–2,811] | 0,890 [0,835–1,020] | 2,330 [1,634–2,779] |

[CSV por nodo y variante](data/r3-region-width-05-11-2026/fast48/per-node.csv) conserva medianas de las fases y del total, y mínimos y máximos de los nueve cocientes por nodo como dispersión observada. Por ejemplo, Majadas 0 con anchura 21 y `fast=true` tiene mediana 0,7753 e intervalo 0,7726–0,8131; Belvedere 10 con anchura 16 tiene 2,7773 y 2,7316–2,8237. Elegir el mínimo entre variantes introduce selección sobre ruido. No se midieron RSS, GPU, navegador, dataset completo ni cambios reales en el tamaño convertido; estos datos no prueban mejoras de producto.

## Procedencia y reproducción

Captura principal: 04-11-2026 23:44:20 UTC, revisión base `3eb78f70f2a90d5a8b7b68d39efc5fd406d2983d`, árbol de trabajo modificado, Ryzen AI 9 365, Linux x86_64, Rust nightly 1.101.0 (41089d07a), perfil `perf`. [Metadatos](data/r3-region-width-05-11-2026/fast48/metadata.txt) contienen SHA-256 de ambos `.oct`, sus `.octattrs`, fuente, manifiesto y binario. El [comando exacto](data/r3-region-width-05-11-2026/fast48/command.sh), [salida comprimida](data/r3-region-width-05-11-2026/fast48/raw.txt.gz), [resumen](data/r3-region-width-05-11-2026/fast48/summary.txt) y [análisis](data/r3-region-width-05-11-2026/fast48/analyze.py) conservan la captura; los scripts contienen rutas locales y deben adaptarse.

La [fuente comprimida](data/r3-region-width-05-11-2026/lab/main.rs.gz), [Cargo.toml](data/r3-region-width-05-11-2026/lab/Cargo.toml) y [Cargo.lock](data/r3-region-width-05-11-2026/lab/Cargo.lock) son los archivos presentes tras la ejecución. La fuente descomprimida coincide con el hash registrado `4988209a4c0b8848661f86d760d2ad0d88b39769ed633f5f8cff4331e42a465c`; el manifiesto coincide con `fc979f9a2da2fc2b07810b4df33e1f5186002949329cccad30d0d75803b7ebfb`, y el binario presente coincide con `82a4dd926c215816b03dece19487647f9fefc7653bdca4d6cb827c2fc60536d6`. El lock tiene SHA-256 `ad9176f167b55e365028eb2b0e44c20bc0d4232d7f1b19e58a82db12a52caea5`, pero no se registró su hash durante la ejecución.

Para reconstruir el laboratorio, colocar estos tres archivos en `target/r3-layout-lab/{src/main.rs,Cargo.toml,Cargo.lock}` de un checkout compatible, ejecutar `cargo build --manifest-path target/r3-layout-lab/Cargo.toml --profile perf --locked`, y adaptar el comando y rutas de datasets. El manifiesto y la fuente referencian crates y código del checkout por rutas relativas. **Límite:** no se archivó el diff exacto de esas dependencias modificado en el momento de la ejecución, ni el binario; por tanto estos archivos no bastan para reconstruirlo bit a bit o repetir íntegramente esa revisión. Los datasets tampoco se distribuyen aquí; hay que comprobar sus hashes y toolchain antes de comparar.

La [captura anterior](data/r3-region-width-05-11-2026/legacy80/metadata.txt) usa otra fuente (`098cdbed…`) y binario (`32cfdbc7…`), y su análisis usa `legacy80` como referencia. Se conservan su [comando](data/r3-region-width-05-11-2026/legacy80/command.sh), [salida](data/r3-region-width-05-11-2026/legacy80/raw.txt.gz), [CSV](data/r3-region-width-05-11-2026/legacy80/per-node.csv), [resumen](data/r3-region-width-05-11-2026/legacy80/summary.txt) y [análisis](data/r3-region-width-05-11-2026/legacy80/analyze.py); esa fuente anterior no está archivada. No mezclar sus cocientes con los de la captura principal.
