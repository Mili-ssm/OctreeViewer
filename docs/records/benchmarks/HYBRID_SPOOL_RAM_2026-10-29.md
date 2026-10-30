# S10: pico de RAM del almacén híbrido de spools (29-10-2026)

> Registro fechado. Modelo sobre trazas nativas más medidas de navegador; cifras válidas solo para este equipo
> (AMD Ryzen AI 9 365, 30 GB de RAM, Linux 7.2.11, `/home` en btrfs), Chrome 156.0.8078.47 y Firefox 157.0 (Playwright
> `firefox-1555`), base `8c97198`. Contexto: [S9](OPFS_SPOOL_FILES_2026-10-29.md) (coste por fichero), el
> [modelo por nodos](NODE_JOB_ARENA_MODEL_2026-10-29.md) (eventos y simulador) y la decisión 2 de la spec
> ([híbrido](../../plans/conversor-web-paralelo.md)). Datos: [JSON del modelo](data/hybrid-spool-ram-2026-10-29.json),
> [JSONL del navegador](data/hybrid-spool-ram-2026-10-29.jsonl), [página](data/hybrid-spool-ram-2026-10-29.html),
> [worker/página JS](data/hybrid-spool-ram-2026-10-29.js), [driver](data/hybrid-spool-ram-2026-10-29-driver.mjs);
> simulador [`model_hybrid_spools.py`](../../../crates/viewer_convert/benchmarks/model_hybrid_spools.py).

## Conclusión

**Una política en la que el worker nunca toca OPFS (P3, «solo el coordinador vuelca») acota el pico de RAM al presupuesto más los segmentos en vuelo y no necesita handles dentro de los trabajos; las políticas con handles abiertos por adelantado (P1, P2) no escalan a 8 divisiones.** El criterio de parada de la enmienda no se activa para P3, pero el sumidero de Firefox no sigue el ritmo de volcado en Majadas (ver «Backpressure»).

| Pregunta | Respuesta |
| --- | --- |
| ¿Cuánto RAM ocupa un segmento retenido? | PSS / bytes retenidos = 1,004–1,03 en Chrome y 0,95–0,99 en Firefox (segmentos de 1 MiB y 32 KiB, 256 MiB–2 GiB). El sobrecoste de los segmentos es ≤ 3 %. |
| ¿Cuesta transferir segmentos al coordinador? | No hay copia: 256 MiB tardan 1–2 ms (Chrome, 1 MiB, un mensaje) a 15–27 ms (32 KiB); un mensaje por segmento, 4–18 ms (1 MiB) a 59–67 ms (32 KiB). |
| P1 (admisión por entrada, handles por trabajo) | RAM ≤ presupuesto (con reserva del peor caso), pero a 8 divisiones cada trabajo volcado abre 513 handles: 11–20 s de aperturas en Firefox, 4.104 handles simultáneos con N = 8 (> 664 de S9) y 85 % vacíos. |
| P3 | RAM del libro ≤ presupuesto en las 36 celdas (× 3 repartos); 0 s de apertura dentro de un trabajo; 1 handle a la vez; con 3 GiB, 300–352 ficheros en Belvedere/2 divisiones, 213–389 con 8 divisiones y 5.168–5.828 en Majadas. |

## Método

- **Trazas.** `real_dataset_bench convert-profile ENTRADA SALIDA 1 DIVISIONES` (perfil `perf`, 1 worker, imprime `internal_spatial_node_event=`) sobre Belvedere con 2 y 8 divisiones y Majadas con 2. Los registros del modelo anterior no se conservaron; se regeneraron (1.343, 5.140 y 6.259 nodos; no hay nodos `became_leaf_all_equal`, que el simulador rechaza). Las duraciones (`elapsed_ns`) vienen de una máquina compartida y valen como pesos relativos.
- **Simulador.** Importa `parse_log`, `source_bytes` y los repartos (FIFO, LIFO, mayor primero) de `model_node_job_arena.py`, y añade su propio bucle porque el original no permite enganchar la política. Contabilidad como la variante c: los spools de hijos entran enteros al empezar el trabajo (cota superior), el LOD al terminar, la entrada se libera al terminar. **Calibración:** con P0 y la entrada de raíz como spool (disposición nativa), el pico coincide byte a byte con `simulate(..., "c")` (LIFO con N = 1 y FIFO con N = 4 en las dos trazas de Belvedere; todos los repartos en la prueba manual; Majadas no se calibró). El caso web no tiene spool de entrada en la raíz (la partición lee el `Blob`): las tablas usan esa disposición.
- **Políticas.** **P0** solo RAM. **P1** admisión por entrada: si `input_spool_bytes + lod_bytes` (peor caso, enmienda del pre-mortem) no cabe en el presupuesto libre, el trabajo recibe de antemano `divisiones³ + 1` handles (513 con 8 divisiones, 9 con 2) y escribe todo a disco; entre trabajos se vuelcan los spools retenidos (hojas y LOD) del mayor al menor hasta bajar del presupuesto. **P1r** (variante propuesta por mí): antes de abrir handles, el host vuelca retenidos y luego entradas en cola (consumidas más tarde primero) para hacer sitio; solo si no basta, handles. **P2** handles en todos los trabajos (referencia de S9). **P3** coordinador único: los workers rellenan segmentos y los envían transferidos; el coordinador lleva el libro de RAM y, al pasarse, abre handles de forma asíncrona y vuelca spools enteros (retenidos, entradas en cola y, al final, salidas aún en escritura, que desde entonces escribe el coordinador). Un trabajo nunca espera un handle. El pico del libro es el del modelo; **RAM real = libro + segmentos en vuelo** (N × K × tamaño de segmento, K = segmentos sin confirmar), acotado por backpressure y no modelado en el tiempo.
- **Handles y empties.** Un trabajo en disco abre `divisiones³ + 1` handles (P1, P1r, P2); «vacíos» = handles que no acaban en un spool con bytes. Un handle vacío se abre, se cierra y se borra: su coste es apertura más `removeEntry` (Firefox 25–40 ms, Chrome 0,5–1,4 ms, p50 de S9). Coste de apertura de S9: Firefox 22–40 ms por handle o fichero, Chrome 2,9–4,4 ms; las tablas dan el extremo alto de Firefox (apertura + borrado de vacíos), en segundos de reloj equivalente, no repartidos entre N. P3 abre un handle por spool volcado y ninguno vacío.
- **Raíz por rangos.** W trabajos por rangos de chunks abren cada uno `divisiones³` handles de hijos (+1 LOD). Los vacíos que se dan son una **cota inferior** (hijos vacíos en todo el conjunto, vacíos en cada rango); los vacíos solo en algunos rangos no se distinguen en la traza.
- **Rejilla.** Presupuestos 0,5/1/2/3 GiB × N = 1, 4, 8 × tres repartos; las tablas muestran el **peor reparto** (máximo) por celda; el JSON tiene todas las celdas y repartos.
- **Navegador.** Página y worker dedicado (`Blob`); segmentos de 32 KiB y 1 MiB rellenos de ruido xorshift (tocan todas las páginas) retenidos hasta 256 MiB, 512 MiB, 1 GiB y 2 GiB. En cada paso la página queda aparcada 2 s y el driver toma la mediana de las muestras de PSS del árbol de procesos del navegador (`examples/web-host/sample-process-memory.py`, intervalo 0,25 s, ≥ 0,6 s después del paso) frente a la línea base sin segmentos. Luego se sueltan los segmentos y se transfieren 256 MiB al hilo principal (a) en un mensaje con todos los segmentos en la lista de transferencia y (b) en un mensaje por segmento. HTTP en 127.0.0.1 (`python3 -m http.server`), perfil nuevo por ejecución bajo `target/s10-profiles` (btrfs, nunca tmpfs), 3 rondas por celda. Antes y después de cada ejecución el driver registra `loadavg` y `/proc/pressure/cpu`: carga 0,85–1,84, `avg10=0.00` en las 24 lecturas; las trazas no se regeneraron en paralelo con el navegador.


## Resultados del modelo

Cada fila es una política y presupuesto; las celdas separan N = 4 y N = 8 (el pico del libro también N = 1). «Pico RAM libro» no incluye segmentos en vuelo. En las tablas el punto separa millares en los recuentos (1.602) y es el decimal en GiB y s. Volcado en GiB. «Coste Firefox» = extremo alto de S9 (s, aperturas más borrado de vacíos). Los ficheros/s usan el reloj relativo de la traza (1 worker nativo): un wasm más lento los baja.


**belvedere-d2** (fuente 3.12 GiB; raíz 4 hijos no vacíos, escribe 3.12 GiB a ≈ 313 MB/s)

| Pol. | Presup. (GiB) | Pico RAM libro N=1 / 4 / 8 (GiB) | Volcado N=4 / 8 (GiB) | Ficheros N=4 / 8 | Handles (vacíos) N=4 / 8 | Handles abiertos a la vez N=4 / 8 | Coste Firefox N=4 / 8 (s, alto) | Ficheros/s N=4 / 8 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| P0 | — | 4.35 / 6.24 / 6.24 | 0.0 / 0.0 | 0 / 0 | 0 (0) / 0 (0) | 0 / 0 | 0 / 0 | 0 / 0 |
| P2 | — | 0.00 / 0.00 / 0.00 | 15.1 / 15.1 | 1.602 / 1.602 | 2.340 (738) / 2.340 (738) | 36 / 72 | 123 / 123 | 112 / 121 |
| P1 | 0.5 | 0.50 / 0.50 / 0.50 | 13.8 / 13.8 | 1.339 / 1.360 | 1.953 (621) / 1.980 (620) | 36 / 72 | 103 / 104 | 93 / 102 |
| P1 | 1 | 1.00 / 1.00 / 1.00 | 12.1 / 12.2 | 1.093 / 1.095 | 1.566 (492) / 1.575 (480) | 36 / 72 | 82 / 82 | 76 / 81 |
| P1 | 2 | 2.00 / 2.00 / 2.00 | 9.4 / 9.6 | 616 / 675 | 891 (275) / 963 (288) | 36 / 72 | 47 / 50 | 43 / 50 |
| P1 | 3 | 3.00 / 3.00 / 3.00 | 5.8 / 6.5 | 149 / 193 | 234 (85) / 297 (105) | 36 / 72 | 13 / 16 | 10 / 15 |
| P1R | 0.5 | 0.50 / 0.50 / 0.50 | 13.1 / 13.4 | 1.148 / 1.279 | 1.176 (34) / 1.347 (122) | 28 / 63 | 48 / 57 | 80 / 96 |
| P1R | 1 | 1.00 / 1.00 / 1.00 | 10.7 / 11.3 | 937 / 996 | 946 (9) / 1.024 (54) | 19 / 46 | 38 / 42 | 65 / 75 |
| P1R | 2 | 2.00 / 2.00 / 2.00 | 7.1 / 8.3 | 535 / 608 | 543 (8) / 645 (65) | 10 / 37 | 22 / 28 | 37 / 46 |
| P1R | 3 | 3.00 / 3.00 / 3.00 | 5.4 / 6.7 | 359 / 390 | 372 (17) / 449 (59) | 9 / 28 | 15 / 20 | 25 / 29 |
| P3 | 0.5 | 0.50 / 0.50 / 0.50 | 13.1 / 13.4 | 1.223 / 1.275 | 1.223 (0) / 1.275 (0) | 1 / 1 | 49 / 51 | 85 / 96 |
| P3 | 1 | 1.00 / 1.00 / 1.00 | 11.2 / 11.3 | 942 / 974 | 942 (0) / 974 (0) | 1 / 1 | 38 / 39 | 66 / 74 |
| P3 | 2 | 2.00 / 2.00 / 2.00 | 7.1 / 7.7 | 542 / 692 | 542 (0) / 692 (0) | 1 / 1 | 22 / 28 | 38 / 52 |
| P3 | 3 | 3.00 / 3.00 / 3.00 | 4.4 / 4.5 | 345 / 352 | 345 (0) / 352 (0) | 1 / 1 | 14 / 14 | 24 / 27 |

Raíz por rangos (handles; vacíos cota inferior; coste Firefox s bajo–alto; Chrome s):
- w1: 9 handles, ≥ 4 vacíos, Firefox 0.3–0.5 s (solo aperturas 0.2–0.4), Chrome 0.0–0.0 s
- w4: 33 handles, ≥ 16 vacíos, Firefox 1.1–2.0 s (solo aperturas 0.7–1.3), Chrome 0.1–0.2 s
- w8: 65 handles, ≥ 32 vacíos, Firefox 2.2–3.9 s (solo aperturas 1.4–2.6), Chrome 0.2–0.3 s

**belvedere-d8** (fuente 3.12 GiB; raíz 81 hijos no vacíos, escribe 3.12 GiB a ≈ 284 MB/s)

| Pol. | Presup. (GiB) | Pico RAM libro N=1 / 4 / 8 (GiB) | Volcado N=4 / 8 (GiB) | Ficheros N=4 / 8 | Handles (vacíos) N=4 / 8 | Handles abiertos a la vez N=4 / 8 | Coste Firefox N=4 / 8 (s, alto) | Ficheros/s N=4 / 8 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| P0 | — | 3.21 / 3.46 / 3.78 | 0.0 / 0.0 | 0 / 0 | 0 (0) / 0 (0) | 0 / 0 | 0 / 0 | 0 / 0 |
| P2 | — | 0.00 / 0.00 / 0.00 | 6.2 / 6.2 | 5.209 / 5.209 | 35.910 (30.701) / 35.910 (30.701) | 2.052 / 4.104 | 2664 / 2664 | 395 / 417 |
| P1 | 0.5 | 0.50 / 0.50 / 0.50 | 5.7 / 5.7 | 4.450 / 4.450 | 32.832 (28.382) / 32.832 (28.382) | 2.052 / 4.104 | 2449 / 2449 | 337 / 356 |
| P1 | 1 | 1.00 / 1.00 / 1.00 | 5.2 / 5.2 | 3.650 / 3.650 | 29.241 (25.591) / 29.241 (25.591) | 2.052 / 4.104 | 2193 / 2193 | 277 / 292 |
| P1 | 2 | 2.00 / 2.00 / 2.00 | 4.2 / 4.2 | 2.060 / 2.060 | 21.033 (19.003) / 21.033 (19.003) | 2.052 / 4.104 | 1601 / 1601 | 156 / 165 |
| P1 | 3 | 3.00 / 3.00 / 3.00 | 3.2 / 3.2 | 295 / 295 | 4.617 (4.359) / 4.617 (4.359) | 2.052 / 3.078 | 359 / 359 | 22 / 24 |
| P1R | 0.5 | 0.50 / 0.50 / 0.50 | 5.7 / 5.7 | 3.125 / 3.629 | 3.556 (431) / 5.948 (2.805) | 513 / 1.540 | 160 / 350 | 235 / 288 |
| P1R | 1 | 1.00 / 1.00 / 1.00 | 5.2 / 5.2 | 2.166 / 2.250 | 2.597 (431) / 2.681 (431) | 513 / 513 | 121 / 124 | 163 / 179 |
| P1R | 2 | 2.00 / 2.00 / 2.00 | 4.2 / 4.2 | 1.010 / 1.025 | 1.441 (431) / 1.456 (431) | 513 / 513 | 75 / 76 | 76 / 82 |
| P1R | 3 | 3.00 / 3.00 / 3.00 | 3.2 / 3.2 | 141 / 141 | 572 (431) / 572 (431) | 513 / 513 | 40 / 40 | 11 / 11 |
| P3 | 0.5 | 0.50 / 0.50 / 0.50 | 5.6 / 5.7 | 3.113 / 4.270 | 3.113 (0) / 4.270 (0) | 1 / 1 | 124 / 171 | 234 / 339 |
| P3 | 1 | 1.00 / 1.00 / 1.00 | 4.6 / 4.9 | 2.359 / 2.343 | 2.359 (0) / 2.343 (0) | 1 / 1 | 94 / 94 | 179 / 186 |
| P3 | 2 | 2.00 / 2.00 / 2.00 | 2.6 / 2.9 | 1.246 / 1.135 | 1.246 (0) / 1.135 (0) | 1 / 1 | 50 / 45 | 94 / 91 |
| P3 | 3 | 3.00 / 3.00 / 3.00 | 0.6 / 0.9 | 239 / 389 | 239 (0) / 389 (0) | 1 / 1 | 10 / 16 | 18 / 31 |

Raíz por rangos (handles; vacíos cota inferior; coste Firefox s bajo–alto; Chrome s):
- w1: 513 handles, ≥ 431 vacíos, Firefox 22.1–37.8 s (solo aperturas 11.3–20.5), Chrome 1.7–2.9 s
- w4: 2.049 handles, ≥ 1.724 vacíos, Firefox 88.2–150.9 s (solo aperturas 45.1–82.0), Chrome 6.8–11.4 s
- w8: 4.097 handles, ≥ 3.448 vacíos, Firefox 176.3–301.8 s (solo aperturas 90.1–163.9), Chrome 13.6–22.9 s

**majadas-d2** (fuente 12.77 GiB; raíz 7 hijos no vacíos, escribe 12.77 GiB a ≈ 317 MB/s)

| Pol. | Presup. (GiB) | Pico RAM libro N=1 / 4 / 8 (GiB) | Volcado N=4 / 8 (GiB) | Ficheros N=4 / 8 | Handles (vacíos) N=4 / 8 | Handles abiertos a la vez N=4 / 8 | Coste Firefox N=4 / 8 (s, alto) | Ficheros/s N=4 / 8 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| P0 | — | 17.53 / 25.54 / 25.54 | 0.0 / 0.0 | 0 / 0 | 0 (0) / 0 (0) | 0 / 0 | 0 / 0 | 0 / 0 |
| P2 | — | 0.00 / 0.00 / 0.00 | 125.5 / 125.5 | 7.527 / 7.527 | 11.421 (3.894) / 11.421 (3.894) | 36 / 72 | 613 / 613 | 87 / 100 |
| P1 | 0.5 | 0.50 / 0.50 / 0.50 | 123.8 / 123.8 | 7.259 / 7.176 | 10.989 (3.730) / 10.899 (3.723) | 36 / 72 | 589 / 585 | 84 / 95 |
| P1 | 1 | 1.00 / 1.00 / 1.00 | 121.5 / 122.0 | 6.973 / 6.914 | 10.584 (3.611) / 10.458 (3.556) | 36 / 72 | 568 / 560 | 81 / 91 |
| P1 | 2 | 2.00 / 2.00 / 2.00 | 114.7 / 115.1 | 6.432 / 6.288 | 9.747 (3.315) / 9.549 (3.261) | 36 / 72 | 522 / 512 | 75 / 83 |
| P1 | 3 | 3.00 / 3.00 / 3.00 | 110.0 / 110.1 | 5.790 / 5.760 | 8.820 (3.030) / 8.739 (2.979) | 36 / 72 | 474 / 469 | 67 / 76 |
| P1R | 0.5 | 0.50 / 0.50 / 0.50 | 123.4 / 112.2 | 7.220 / 6.939 | 7.516 (331) / 7.401 (470) | 36 / 72 | 312 / 314 | 84 / 92 |
| P1R | 1 | 1.00 / 1.00 / 1.00 | 119.8 / 105.0 | 6.714 / 6.394 | 6.857 (196) / 6.857 (463) | 36 / 64 | 280 / 293 | 78 / 84 |
| P1R | 2 | 2.00 / 2.00 / 2.00 | 106.2 / 90.0 | 6.108 / 5.839 | 6.160 (76) / 6.047 (281) | 36 / 63 | 248 / 250 | 71 / 77 |
| P1R | 3 | 3.00 / 3.00 / 3.00 | 94.1 / 81.8 | 5.686 / 5.225 | 5.721 (51) / 5.401 (219) | 36 / 63 | 230 / 223 | 66 / 69 |
| P3 | 0.5 | 0.50 / 0.50 / 0.50 | 124.1 / 116.4 | 7.254 / 6.944 | 7.254 (0) / 6.944 (0) | 1 / 1 | 290 / 278 | 84 / 92 |
| P3 | 1 | 1.00 / 1.00 / 1.00 | 122.0 / 105.4 | 6.783 / 6.405 | 6.783 (0) / 6.405 (0) | 1 / 1 | 271 / 256 | 79 / 85 |
| P3 | 2 | 2.00 / 2.00 / 2.00 | 111.4 / 88.2 | 6.209 / 5.677 | 6.209 (0) / 5.677 (0) | 1 / 1 | 248 / 227 | 72 / 74 |
| P3 | 3 | 3.00 / 3.00 / 3.00 | 101.0 / 76.9 | 5.828 / 5.168 | 5.828 (0) / 5.168 (0) | 1 / 1 | 233 / 207 | 68 / 68 |

Raíz por rangos (handles; vacíos cota inferior; coste Firefox s bajo–alto; Chrome s):
- w1: 9 handles, ≥ 1 vacíos, Firefox 0.2–0.4 s (solo aperturas 0.2–0.4), Chrome 0.0–0.0 s
- w4: 33 handles, ≥ 4 vacíos, Firefox 0.8–1.5 s (solo aperturas 0.7–1.3), Chrome 0.1–0.2 s
- w8: 65 handles, ≥ 8 vacíos, Firefox 1.6–2.9 s (solo aperturas 1.4–2.6), Chrome 0.2–0.3 s

## Resultados del navegador

Tres rondas por celda, mediana (y rango). Ratio = (PSS del árbol − línea base) / bytes retenidos. Línea base: Chrome 390–394 MiB, Firefox 478–498 MiB (árbol de 12 y 8 procesos).

| Navegador | Segmento | 256 MiB | 512 MiB | 1 GiB | 2 GiB |
| --- | --- | --- | --- | --- | --- |
| Chrome 156 | 32 KiB | 1,029 (1,028–1,030) | 1,018 (1,017–1,019) | 1,014 (1,013–1,014) | 1,011 (1,010–1,011) |
| Chrome 156 | 1 MiB | 1,020 (1,019–1,022) | 1,011 (1,008–1,012) | 1,007 (1,005–1,007) | 1,004 (1,003–1,004) |
| Firefox 157 | 32 KiB | 0,968 (0,965–0,969) | 0,982 (0,982–0,984) | 0,986 (0,983–0,987) | 0,991 (0,991–0,991) |
| Firefox 157 | 1 MiB | 0,949 (0,946–0,952) | 0,969 (0,949–0,969) | 0,976 (0,963–0,977) | 0,980 (0,980–0,980) |

Los ratios de Firefox bajan de 1 porque la línea base incluye páginas que luego se comparten o se reorganizan; no son una ganancia real (RSS da 0,97–0,997). Coste de rellenar los segmentos: 236–409 µs por MiB en Chrome y 250–395 µs en Firefox (incluye el `set` de ruido).

Transferencia de 256 MiB del worker al hilo principal (ms, 3 rondas; `envío` = tiempo de `postMessage` en el worker, `total` = hasta recibir el último segmento):

| Navegador | Segmento | Un mensaje: envío / total | Un mensaje por segmento: envío / total |
| --- | --- | --- | --- |
| Chrome 156 | 32 KiB (8.192 seg.) | 11,7–17,6 / 14,8–26,5 | 16,1–28,8 / 59,1–67,0 |
| Chrome 156 | 1 MiB (256 seg.) | 1,0–1,7 / 0,8–2,0 | 1,1–2,0 / 4,0–17,8 |
| Firefox 157 | 32 KiB | 2–3 / 5–7 | 13–23 / 28–56 |
| Firefox 157 | 1 MiB | 0–1 / 2–4 | 1–2 / 3–4 |

La transferencia no copia: tras recibirla, el PSS sube lo que ocupa el lote (Chrome +257–282 MiB sobre la línea base, Firefox +210–217 MiB, salvo una lectura de 476 MiB en una ronda de Firefox 32 KiB donde aún no se había recogido el lote anterior). Tras soltar los 2 GiB, **Chrome conserva 2.055–2.070 MiB** hasta que el recolector actúa (Firefox los libera: −41 a −47 MiB respecto a la línea base), así que los workers de spools no se pueden reciclar por memoria en Chrome sin forzar su cierre.

## Backpressure: ¿hace falta con P3?

Sí en Firefox, no con seguridad en Chrome. Comparación entre la tasa de volcado que pide el modelo (tablas, ficheros/s y MB/s de volcado sobre el reloj de la traza) y la capacidad medida en S9:

| Magnitud | Pide el modelo | Capacidad S9 |
| --- | --- | --- |
| Ficheros/s, Belvedere/2 div., 2 GiB | 38 (N = 4) a 52 (N = 8) | Firefox ≈ 25–45 (22–40 ms por handle, serie), Chrome ≈ 230–340 |
| Ficheros/s, Belvedere/8 div., 2 GiB | 94 / 91 | idem |
| Ficheros/s, Majadas, 2 GiB | 72 / 74 | idem |
| MB/s, escritura de la raíz | ≈ 284–317 (1 worker) | Firefox ≈ 150–250 (casi independiente del tamaño), Chrome ≈ 1.500–2.100 |

Con 3 GiB Belvedere/2 divisiones pide 24–27 ficheros/s (en el límite de Firefox); con 2 GiB o menos y en Majadas el coordinador de Firefox se queda corto y los workers esperarán el acuse tras K segmentos. En Majadas, 5.168–7.254 ficheros a 22–40 ms suman 207–290 s de aperturas en serie, comparables a los 222 s del CLI nativo: Firefox sería el cuello de botella de toda la conversión. Chrome (2,9–4,4 ms por fichero) necesita 15–32 s y no se bloquea. Un wasm más lento que el nativo baja estas tasas pero no el total de ficheros.

## Criterio de parada de la enmienda

«Si toda política cuesta > 1 s de apertura por trabajo volcado en Firefox, o necesita más de los 664 handles simultáneos de S9, el diseño de spools de M3b se replanifica con el propietario.»

| Política | Apertura por trabajo volcado (Firefox) | Handles simultáneos (máx. N = 8) | ¿Supera? |
| --- | --- | --- | --- |
| P1, P2 | 0,2–0,4 s con 2 divisiones; **11,3–20,5 s con 8** | 72 (2 div.); **4.104 (8 div.)** | Sí con 8 divisiones |
| P1r | ídem en los pocos trabajos en disco (1–7 con 8 div.) | 63 (2 div.); 513–1.540 (8 div.) | Sí con 8 divisiones y N = 8, presupuesto 0,5 GiB |
| P3 | 0 s dentro del trabajo (el coordinador abre en segundo plano, 22–40 ms por spool volcado) | 1 | **No** |

**No toda política supera el umbral: P3 lo cumple, así que el criterio no obliga a replanificar el diseño.** Queda el riesgo de rendimiento de Firefox en Majadas descrito arriba, que sí es una decisión para el propietario (ver «Decisión propuesta»). Raíz por rangos con 8 divisiones: 513 handles con 1 rango, 2.049 con 4 y 4.097 con 8 (≥ 84 % vacíos), 88–151 s y 176–302 s en Firefox, 7–11 s y 14–23 s en Chrome; con 2 divisiones, 33 y 65 handles y 1,1–3,9 s en Firefox. Pre-abrir los handles de los hijos en un trabajo por rangos solo es viable con pocas divisiones; con 8, P3 evita abrir los vacíos.

## Decisión propuesta

(La decide el propietario; esto es una propuesta.)

| Elemento | Propuesta | Motivo |
| --- | --- | --- |
| Política | **P3**: el worker nunca toca OPFS; el coordinador lleva el libro y vuelca spools enteros (retenidos primero, luego entradas en cola, al final salidas aún en escritura) con handles asíncronos | Sin apertura dentro del trabajo, 1 handle simultáneo, sin handles vacíos; el libro no pasa del presupuesto en las 108 celdas (36 × 3 repartos) |
| Tamaño de segmento | **1 MiB** (32 KiB solo si el núcleo lo exige) | Ratio de PSS 1,004–1,02; 1 MiB transfiere 256 MiB en 1–2 ms (un mensaje) frente a 15–27 ms con 32 KiB, y 256 mensajes frente a 8.192 |
| Presupuesto inicial de spools | **2 GiB** (parámetro de `ram_budget`, R9) | 3 GiB baja Belvedere/2 divisiones a ≈ 350 ficheros pero deja menos para wasm y la pestaña; 2 GiB queda en 540–690 ficheros (Belvedere/2 div.) y ≈ 1.100–1.250 (8 div.). Por debajo de 1 GiB los ficheros se acercan a los 5.200 de P2 |
| Backpressure | Un worker espera el acuse tras **K = 8** segmentos sin confirmar | Con 1 MiB y N = 8 son 64 MiB en vuelo |
| Fórmula del pico (M3b/M6) | `RAM_spools ≈ 1,03 × (B + N × K × S)`; con B = 2 GiB, K = 8, S = 1 MiB: N = 4 → 2,1 GiB, N = 8 → 2,2 GiB. Pico del proceso ≈ `RAM_spools` + línea base del navegador (0,4–0,5 GiB) + N × memoria wasm del trabajo (0,5–1 GiB por la spec) | El libro nunca supera B; los segmentos en vuelo son el único exceso y el ratio medido es ≤ 1,03 |

Riesgos para el propietario: (1) en Firefox, 5.168–7.254 ficheros en Majadas cuestan ≈ 207–290 s de aperturas en serie con el coste de S9 y los workers esperarán por backpressure; S9 no probó mitigaciones (agrupar spools pequeños en un fichero con extents, abrir en lotes). (2) Volcar una salida aún en escritura exige que el coordinador siga escribiendo en el fichero los segmentos que lleguen después; es un detalle de diseño que este modelo supone. (3) La política alternativa P1r (hacer sitio antes de abrir handles) también cumple el RAM, pero con 8 divisiones fuerza un trabajo con 513 handles y con N = 8 hasta 1.540 simultáneos.

## Límites

- Una traza por caso (1 worker, perfil `perf`); `elapsed_ns` de una máquina con otras cargas, solo pesos relativos. Majadas solo con 2 divisiones. Los tiempos de volcado, la copia de los segmentos y la lectura de entradas en disco no se modelan; el modelo no tiene reloj real, solo orden y concurrencia, así que **el efecto del backpressure no está medido**: las tasas son un balance, no una simulación temporal.
- Los segmentos en vuelo (N × K × S) se suman por fórmula; no se midió un pipeline real de postMessage con acuses.
- Las salidas de un trabajo se cuentan enteras al empezar (cota superior); en P3 el libro vuelca salidas sin terminar, lo que presupone el seguimiento descrito en el riesgo 2.
- Navegador: una instancia headless de Chrome 156 y de Firefox 157, 3 rondas por celda, solo Linux; PSS del árbol, no heap JS. Safari/WebKit y Firefox 158 no se midieron. El hilo principal retiene los lotes transferidos, pero no se midió la presión de memoria de un sistema casi lleno.
- Los costes de apertura vienen de S9 (una ejecución por celda, árbol sintético); se aplican como multiplicadores lineales sin contención.
- Pico de P0 con la disposición web: no cuenta el spool de entrada de la raíz; las tablas P0 son una cota de lo que un P0 necesitaría (6,24 GiB Belvedere/2 div., 3,46–3,78 GiB con 8, 17,5–25,5 GiB Majadas).

## Datos y limpieza

Trazas y perfiles bajo `target/` (`s10-traces`, `s10-profiles`, `s10-samples`; perfiles y muestras borrados por el driver, `profileRemoved: true` en las 12 ejecuciones). El JSON del modelo contiene todas las celdas por reparto; el JSONL solo las medianas de PSS por paso, los lotes de transferencia y `loadavg`/presión de CPU.
