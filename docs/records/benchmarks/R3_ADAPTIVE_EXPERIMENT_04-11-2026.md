# Experimento CPU de celdas adaptativas — 04-11-2026

**Observado:** Majadas completa con celdas adaptativas, pero Belvedere falla con ambas variantes por necesitar más de 21 bits relativos por eje. El experimento no se promueve: no prueba R3 en navegador ni GPU. La comparación V5 mide la evolución completa de la rama, no el efecto de las celdas adaptativas.

## Identidad y alcance

Equipo `pc-tatil`, Linux 7.2.15-300.fc45.x86_64, perfil `perf`, toolchain `nightly-2026-10-28` (detalles en [identity](data/r3-adaptive-experiment-04-11-2026/identity)). La referencia V5 es `ded7ee839408f2d1b8a6ea2748ff1263e1d45598` limpia; la rama experimental parte de `3eb78f70f2a90d5a8b7b68d39efc5fd406d2983d`. Entre ambas hay muchos cambios, no un A/B aislado de R3.

La referencia fija de celdas es trabajo sin commit sobre `3eb78f`: modificaciones en `crates/viewer_render_wgpu/examples/node_cells_bench.rs`, `src/node_cells.rs` y `src/node_cells_tests.rs`. Su [parche](data/r3-adaptive-experiment-04-11-2026/fixed-source.patch.gz) y [hashes de fuente](data/r3-adaptive-experiment-04-11-2026/source-sha256.txt) coinciden con la captura anterior a las medidas. El [parche adaptativo](data/r3-adaptive-experiment-04-11-2026/adaptive-source.patch.gz) es una captura posterior; no se conservaron hashes de fuente de esa variante en el momento de compilación. Los binarios y datasets se identifican por SHA-256 en [binarios](data/r3-adaptive-experiment-04-11-2026/binary-sha256-all.txt), [datasets v7.5](data/r3-adaptive-experiment-04-11-2026/dataset-sha256.txt) e identidades de cada ejecución V5. Los binarios y datasets no se incluyen en este registro.

## V5: referencia frente a rama experimental

Majadas V5 (`output.oct` SHA-256 `742262867c844bb4e876cd4280a648e45ee677624a3cebd1a8d71bbbf6ff6f04`; `.octattrs` `d45cd5fd70816d24ea29137faaf4ec1ca6cc2a4fcdc8be2dabbefc20f9a543f1`). Tras calentamiento, tres parejas alternadas, cada ejecución con tres construcciones; tiempos son la mediana interna de cada ejecución. RSS es el máximo del proceso medido por `/usr/bin/time -v` en KiB.

| Pareja | Referencia s / RSS | Experimental s / RSS | Diferencia de tiempo |
|---|---:|---:|---:|
| 1 | 0,286996 / 131928 | 0,297289 / 132932 | +3,59 % |
| 2 | 0,290430 / 131516 | 0,296168 / 132700 | +1,98 % |
| 3 | 0,293112 / 131936 | 0,301633 / 132684 | +2,91 % |

La mediana de diferencias relativas emparejadas es **+2,91 %**. En las seis ejecuciones coinciden digest de escena `563a1f928baa5a75`, 6397 vóxeles y 202 bloques seleccionados. Eso demuestra igualdad de esos indicadores, no equivalencia visual completa. [Salidas e identidades](data/r3-adaptive-experiment-04-11-2026/measurements/v5-compatible/) conservan muestras, orden y carga.

## Celdas sobre v7.5

| Dataset | Fijo | Adaptativo |
|---|---|---|
| Majadas | 386,00 s, 219320 KiB; salida parcial y código 1 por colisiones de posiciones distintas en el límite de profundidad | 305,97 s, 218504 KiB; código 0, cero hojas con esas colisiones |
| Belvedere | código 1: nodo requiere más de 21 bits relativos por eje | mismo error y nodo, código 1 |

Solo hay una ejecución por variante para Majadas; las condiciones de E/S difieren y el fijo termina con error, por lo que no se deduce rendimiento. Belvedere no completa. [Salidas, errores y tiempos](data/r3-adaptive-experiment-04-11-2026/measurements/node-cells/) incluyen el segundo intento fijo incompleto. El intento V5 provisional separado falló con `ProvisionalFormat`; sus [logs](data/r3-adaptive-experiment-04-11-2026/measurements/v5/) no forman parte de la comparación.

## Reproducción y límites

Con los binarios y datasets identificados disponibles en las rutas originales, las líneas `argv` de cada archivo `.identity` reproducen las invocaciones; anteponer `/usr/bin/time -v` para RSS. Para V5 ejecutar calentamiento y tres parejas en el orden registrado, con argumento `3`; para celdas pasar el `.oct` correspondiente. Verificar primero con `sha256sum` los binarios y ambos archivos del dataset. Las rutas son locales y los binarios no están archivados: reconstruirlos exige recuperar las fuentes y el entorno exactos, y comprobar su hash; el parche adaptativo posterior no prueba por sí solo la fuente compilada.

No hay aquí pruebas de GPU, navegador, presentación visual ni rendimiento causal de R3. Las mediciones describen únicamente este equipo, datasets, perfil y revisiones.
