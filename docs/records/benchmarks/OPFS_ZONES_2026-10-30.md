# S5 residual: zonas OPFS, `truncate`, liberación de handles y RAM de JS (30-10-2026)

> Registro fechado. Cifras válidas solo para este equipo (Linux 7.2.13, 30 GB de RAM, `/home` en btrfs
> con `compress=zstd:1`, NVMe), Chrome 156.0.8078.73 (`/usr/bin/google-chrome`, headless) y Firefox 157.0
> (Playwright `firefox-1555`). Tres rondas por celda; se da mediana y [rango]. Contexto: Tarea 1 del
> [plan de la etapa 4](../archive/conversor-web-paralelo-etapa4.md) (decisión 5, D1 «zonas OPFS
> paginadas»); antecedentes [S9](OPFS_SPOOL_FILES_2026-10-29.md), [S8](OPFS_QUOTA_2026-10-29.md),
> [S1](OPFS_ARENA_SPIKE_2026-10-29.md) y [S10](HYBRID_SPOOL_RAM_2026-10-29.md). Datos crudos:
> [JSONL](data/opfs-zones-2026-10-30.jsonl); [HTML](data/opfs-zones-2026-10-30.html),
> [JS](data/opfs-zones-2026-10-30.js), [driver](data/opfs-zones-2026-10-30-driver.mjs).

## Conclusión

**OPFS soporta lo que exigen las zonas en los tres entornos; ningún criterio de parada se activa.** Un fichero de 6 GiB
se escribe y se lee con `at` por encima de 2³² sin errores ni bytes distintos; `truncate` libera el disco de verdad
(Chrome solo actualiza `estimate().usage` al cerrar el handle); Firefox escribe a miles de MB/s, no a < 100 MB/s;
y el handle de un worker terminado se libera en ≈ 2,0 s en Chrome y ≤ 0,16 s en Firefox, nunca más de 5 s.

| Criterio de parada del coordinador | Resultado |
| --- | --- |
| Lectura/escritura > 2³² falla, o `truncate` no baja el uso → acotar zonas < 4 GiB | **No se activa.** 0 páginas distintas de 6.144 leídas (9 ejecuciones); 42 de 128 lecturas aleatorias por encima de 4 GiB, más la última página y una lectura que cruza 2³² (65.536 B); `truncate` baja `du` de inmediato y `estimate().usage` en Firefox al instante y en Chrome tras `close()` (ver tabla 2). |
| Firefox escribe ruido a < 100 MB/s, o un handle tarda > 5 s en liberarse tras `terminate()` | **No se activa.** Firefox: 3.912–4.230 MB/s (mediana) a 1 MiB, ≥ 719 MB/s a 32 KiB; liberación máxima observada: 156 ms (Firefox) y 2.012 ms (Chrome); 0 fallos en 90 intentos por navegador/origen. |

## Método

- **Páginas y worker.** Un worker dedicado (Blob URL, funciona por `file://`) abre un `FileSystemSyncAccessHandle` y escribe 6 GiB en 6.144 páginas de 1 MiB de ruido xorshift32 sembrado por número de página (incompresible y verificable). El tiempo de relleno se excluye de los MB/s (en la tabla, MB/s = 10⁶ B/s). Lectura secuencial con `read(buf, {at})` de las 6.144 páginas, **todas** comparadas con la regeneración esperada (la comparación no entra en el MB/s); luego 128 lecturas aleatorias (incluidas las páginas 0, 4.095–4.097 y la última; 42 por encima de 2³²) y una lectura de 64 KiB cruzando 2³².
- **Cuota y disco.** Tras escribir, tras leer, tras `truncate(1 GiB)`+`flush`, tras `truncate(0)`+`flush` y tras `removeEntry`, la página se detiene y el driver mide a los 2 s y a los 12 s `df` de `/home`, `du -sk` del perfil y `estimate().usage`. Una prueba aparte (`truncq`, 2 GiB, 3 rondas) añade el paso «tras `close()`».
- **Escrituras pequeñas.** 1 GiB por granularidad (32 KiB, 64 KiB, 256 KiB, 1 MiB) con `write({at})` secuencial sobre ruido pregenerado de 16 MiB (a diferentes desplazamientos), sin relleno en el tiempo.
- **`terminate()`.** El worker abre 1 handle (`rm1`, `open1`) u 8 (`rm8`) y escribe páginas de 1 MiB en bucle síncrono; al llegar a la escritura 40 la página llama a `worker.terminate()` y reintenta cada 5 ms hasta 10 s `removeEntry` (`rm1`, `rm8`: tiempo hasta borrar todos) o `createSyncAccessHandle` desde un worker nuevo (`open1`). 10 intentos por ronda y variante.
- **RAM de JS.** Un worker retiene hasta 4 GiB de `ArrayBuffer` de 32 KiB, 256 KiB o 1 MiB (cada página tocada con ruido) y se detiene en 1, 2, 3 y 4 GiB; el driver muestrea el árbol de procesos con `examples/web-host/sample-process-memory.py` (PSS, 0,25 s) y toma la mediana de las muestras ≥ 0,6 s tras cada paso. Razón = (PSS − PSS de la línea base) / bytes retenidos. **Rotación:** un bucle síncrono sin `await` reserva 8 GiB de segmentos de 256 KiB manteniendo ≤ 2 GiB vivos (FIFO), en dos variantes (`Uint8Array` nuevo por segmento; pool fijo de segmentos reutilizados); pico de PSS durante el bucle y PSS tras liberar (inmediato y a los 8 s).
- **Entorno y aislamiento.** Perfil persistente nuevo por ejecución bajo `target/s5-profiles` (btrfs, nunca tmpfs), servidor `python3 -m http.server` en 127.0.0.1, borrado en `finally`. Celdas: Chrome http, Firefox http, Firefox `file://` (Chrome `file://` no tiene OPFS). Se registran `loadavg` y PSI de CPU por ejecución; el driver espera a `loadavg` < 2 antes de empezar. Máximo de `loadavg` (1 min) al empezar una ejecución: 1,97; PSI de CPU `avg10` siempre 0,00. El loadavg final llegó a 2,23 por el propio navegador. Otro agente ligero (modelo en Python) pudo estar activo; no se aprecia en el PSI.

## Resultados

Tabla 1: fichero de 6 GiB (mediana [rango] de 3 rondas; MB/s = 10⁶ B/s). **Son velocidades de caché de página, no de disco** (ver límites).

| Medida | Chrome 156 http | Firefox 157 http | Firefox 157 file |
| --- | --- | --- | --- |
| Escritura 1 MiB (MB/s) | 17.620 [16.720–18.730] | 4.230 [3.686–6.581] | 3.912 [3.458–4.844] |
| Lectura secuencial (MB/s) | 13.760 [13.540–14.330] | 10.880 [8.145–12.660] | 10.770 [10.290–12.290] |
| Lectura aleatoria de páginas (MB/s) | 12.660 [12.540–15.430] | 11.180 [6.711–12.200] | 8.948 [7.457–8.948] |
| `flush` tras 6 GiB (ms) | 283 [185–742] | 452 [406–506] | 510 [383–659] |
| Páginas con bytes distintos / escrituras cortas | 0 / 0 | 0 / 0 | 0 / 0 |
| Lecturas aleatorias > 2³² correctas | 42 de 42 | 42 de 42 | 42 de 42 |
| Reescribir 256 MiB tras `truncate(0)` (ms) | 239 [222–252] | 800 [779–951] | 874 [866–1.070] |
| `truncate(1 GiB)`/`truncate(0)` (ms) | 23 / 6 | 18 / 6 | 19 / 6 |
| `removeEntry` del fichero (ms) | 1,7 | 10 | 9 |

Tabla 2: `truncate` y cuota (los tres entornos y las tres rondas coinciden salvo el `df`). `du` del perfil en GiB; `estimate().usage` en MiB.

| Paso | Chrome `du` / `usage` | Firefox `du` / `usage` (http y file) |
| --- | --- | --- |
| Escrito (6 GiB) | 6,00 / 6.144 | 6,03 / 6.144 |
| `truncate(1 GiB)` + `flush` | 1,00 / **6.144** (también a los 12 s) | 1,03 / 1.024 |
| `truncate(0)` + `flush` | 0,00 / **6.144** (también a los 12 s) | 0,03 / 0 |
| Tras `close()` (prueba `truncq`, 2 GiB) | 0,00 / **0** | 0,03 / 0 |
| Tras `removeEntry` | 0,00 / 0 | 0,03 / 0 |

`df` de `/home` no sirve como medida: btrfs contabiliza con retraso y la máquina no estaba aislada, así que
las variaciones por paso son incoherentes entre rondas (p. ej. 0 GiB y 6 GiB tras escribir en rondas distintas);
el JSONL las conserva. `du` del perfil es la medida de disco de este registro. La cuota inicial fue de 10 GiB en
ambos navegadores (Chrome informó 11,8 GiB con 6 GiB de uso).

Tabla 3: escrituras pequeñas, `write({at})` secuencial, MB/s mediana [rango] de 3 rondas. Aprox. 36–40 µs por escritura en Firefox a 32 KiB.

| Granularidad | Chrome 156 http | Firefox 157 http | Firefox 157 file |
| --- | --- | --- | --- |
| 32 KiB | 3.668 [3.653–3.709] | 846 [747–902] | 846 [719–898] |
| 64 KiB | 7.319 [5.506–7.971] | 1.405 [1.382–1.473] | 1.598 [1.309–1.829] |
| 256 KiB | 10.920 [10.810–11.350] | 4.052 [3.741–4.278] | 3.677 [3.665–4.227] |
| 1 MiB | 9.536 [9.345–11.670] | 7.725 [7.405–8.073] | 7.111 [6.753–7.255] |

Tabla 4: liberación tras `terminate()` (ms hasta el éxito; 30 intentos por celda: 10 × 3 rondas).

| Variante | Chrome 156 http (mín / p50 / p90 / máx) | Firefox 157 http | Firefox 157 file | Fallos > 5 s |
| --- | --- | --- | --- | --- |
| `removeEntry`, 1 handle | 2.006 / 2.009 / 2.010 / 2.011 | 16 / 18 / 21 / 21 | 16 / 18 / 21 / 25 | 0 |
| `createSyncAccessHandle` desde worker nuevo, 1 handle | 2.001 / 2.004 / 2.006 / 2.007 | 12 / 14 / 15 / 16 | 12 / 14 / 16 / 16 | 0 |
| `removeEntry`, 8 handles (hasta el último) | 2.007 / 2.010 / 2.012 / 2.012 | 125 / 140 / 146 / 150 | 128 / 142 / 148 / 156 | 0 |

El único error visto es `NoModificationAllowedError`. En Chrome el tiempo es ≈ 2,00 s con dispersión de 10 ms en todos los intentos: parece un temporizador fijo de la limpieza del worker, no una carrera. Se hizo `rm8` con 8 ficheros de un solo worker; con 664 handles (S9) no se probó.

Tabla 5: razón PSS / bytes retenidos (mediana [rango]) con segmentos de JS retenidos en un worker. **No hubo ningún fallo de reserva hasta 4 GiB** (sin `RangeError` ni otra excepción) en las 27 ejecuciones.

| Navegador | Segmento | 1 GiB | 2 GiB | 3 GiB | 4 GiB |
| --- | --- | --- | --- | --- | --- |
| Chrome 156 | 32 KiB | 1,016 [1,016–1,017] | 1,013 [1,012–1,013] | 1,011 | 1,011 [1,010–1,011] |
| Chrome 156 | 256 KiB | 1,008 | 1,005 | 1,004 | 1,003 |
| Chrome 156 | 1 MiB | 1,008 [1,008–1,009] | 1,006 | 1,005 | 1,004 |
| Firefox 157 http | 32 KiB | 1,000 | 1,005 | 1,000 [0,999–1,001] | 1,003 [1,001–1,003] |
| Firefox 157 http | 256 KiB | 0,998 [0,998–1,010] | 1,003 [1,003–1,009] | 1,001 [0,999–1,005] | 1,001 [1,001–1,004] |
| Firefox 157 http | 1 MiB | 0,987 | 0,992 | 0,991 | 0,990 |
| Firefox 157 file | 32 KiB | 1,000 | 1,005 | 1,003 | 1,003 |
| Firefox 157 file | 256 KiB | 0,998 | 1,003 | 1,001 | 1,001 |
| Firefox 157 file | 1 MiB | 0,988 | 0,992 | 0,991 | 0,989 |

Tabla 6: rotación (8 GiB reservados, ≤ 2 GiB vivos, segmentos de 256 KiB, bucle síncrono; PSS en GiB, mediana de 3 rondas; la línea base del navegador es 0,39 en Chrome y 0,49 en Firefox, así que «vivo + base» es 2,39 y 2,49). Sin `RangeError` en ninguna ejecución.

| Navegador | Variante | Tiempo del bucle (ms) | Pico de PSS | PSS tras liberar | PSS 8 s después |
| --- | --- | --- | --- | --- | --- |
| Chrome 156 | `Uint8Array` nuevo | 2.668 [2.262–2.729] | 2,76 [2,70–2,77] | 2,41 [2,41–2,52] | 2,40 [2,40–2,51] |
| Chrome 156 | pool fijo | 727 [636–913] | 2,41 [2,40–2,41] | 2,41 | 2,40 |
| Firefox 157 http | `Uint8Array` nuevo | 1.445 [1.127–1.448] | 3,51 [3,50–3,51] | 2,47 | 0,42 |
| Firefox 157 http | pool fijo | 908 [720–924] | 2,50 [2,50–2,51] | 0,47–2,48 | 0,43–0,44 |
| Firefox 157 file | `Uint8Array` nuevo | 1.478 [1.473–1.479] | 3,52 | 2,49 | 0,44 |
| Firefox 157 file | pool fijo | 736 [635–922] | 2,51 [2,50–2,52] | 0,47–2,50 | 0,44 |

## Lectura

- **Zonas grandes y `truncate`.** Un fichero de 6 GiB (> 2³²) funciona con páginas de 1 MiB en los tres entornos. `truncate` libera el disco en cuanto vuelve (`du`). La **cuota que informa Chrome no baja hasta cerrar el handle** (`usage` quedó en 6.144 MiB con el handle abierto, incluso 12 s después de `truncate(0)`+`flush`); Firefox la baja al instante. Para el control de cuota de las zonas, Chrome exige cerrar y reabrir el handle (o no confiar en `estimate()` mientras haya handles abiertos).
- **MB/s no son de disco.** La escritura de ruido va a la caché de página de btrfs (RAM libre ≈ 10 GB y 20 GB disponibles) y `flush` tarda 0,2–0,7 s, lo que no es un `fsync` completo de 6 GiB; sirven como cota del coste de CPU/IPC de OPFS, no del rendimiento sostenido de disco. Aun así, el criterio de 100 MB/s de Firefox se supera en un orden de magnitud con holgura.
- **Escrituras pequeñas.** Hasta 256 KiB las curvas son lineales en el tamaño de escritura: Firefox paga ≈ 36–40 µs por escritura (846 MB/s a 32 KiB, 4 GB/s a 256 KiB, 7,7 GB/s a 1 MiB); Chrome llega a ≈ 3,7 GB/s con 32 KiB. Con 32 KiB Firefox rinde ≈ 9 × menos que con 1 MiB; un volcado que escriba en trozos ≥ 256 KiB queda en la meseta de Chrome y a la mitad de la de Firefox.
- **Liberación del handle.** Chrome libera siempre a los ≈ 2,0 s (dispersión ≤ 12 ms; el reintento cada 5 ms hizo ≈ 390 intentos), Firefox a 12–25 ms (1 handle) o ≈ 140 ms (8 handles, ≈ 17 ms por handle). Los 8 handles de un worker se liberan juntos en Chrome.
- **RAM.** PSS / bytes ≈ 1,00–1,02 en Chrome y 0,99–1,01 en Firefox desde 1 hasta 4 GiB: los segmentos cuestan lo que pesan, sin sobrecoste apreciable por el tamaño (32 KiB cuesta +1 % en Chrome). Un worker puede retener 4 GiB sin excepción.
- **Rotación.** Con asignación nueva, el pico de PSS supera lo vivo en +0,35 GiB (Chrome) y +1,0 GiB (Firefox, recolector tras el bucle síncrono). Con pool fijo el pico es lo vivo más la base en ambos. Después de liberar, **Chrome conserva ≈ 2 GiB** (PSS no baja ni a los 8 s, como en S10), también con pool; Firefox los devuelve en unos segundos (0,42–0,44 a los 8 s). Un pool no ayuda a Chrome a devolver memoria, solo evita el pico por encima de lo vivo.

## Decisión propuesta

(El coordinador decide.)

| Tema | Propuesta | Razón |
| --- | --- | --- |
| Tamaño de zona | **1 GiB** (máximo 2 GiB), no cerca de 4 GiB. | 6 GiB funciona, así que no hay tope técnico; la zona pequeña libera cuota por zona con `truncate` y acota la pérdida si una zona queda en un estado raro. Handles por abrir: Majadas (≈ 25,5 GiB de pico) → ≈ 26 zonas, a 22–40 ms por handle en Firefox (S9) ≈ 1 s. Abrir todas antes de convertir es barato. |
| Tamaño de página | **1 MiB**, con escrituras de volcado agrupadas en ≥ 256 KiB. | 256 KiB–1 MiB están en la meseta; a 32 KiB Firefox rinde ≈ 9 × menos. Un spool que sigue añadiendo tras volcarse debería acumular en RAM hasta 256 KiB antes de escribir. |
| Cuota | Tras `truncate`, **cerrar y reabrir** el handle (o contabilizar por la propia tabla de páginas) para ver bajar `usage` en Chrome. | `usage` de Chrome solo baja al cerrar. |
| Reintentos de limpieza tras `terminate()` | Reintentar `removeEntry`/`createSyncAccessHandle` ante `NoModificationAllowedError` **cada 50 ms hasta 10 s** (observado: ≤ 2,02 s en Chrome, ≤ 0,16 s en Firefox); pasado el plazo, dejar la zona marcada para limpiar en el siguiente arranque. | Margen de 5 × sobre el peor caso medido; una reconversión al mismo nombre debe esperar o usar un nombre de zona nuevo (≥ 2 s en Chrome). |
| Segmento de RAM de JS | **256 KiB** (1 MiB para spools grandes ya crecidos); pool de tamaños fijos reutilizado entre trabajos. | PSS/bytes ≤ 1,008 a 256 KiB y 1 MiB, 1,016 a 32 KiB; sin fallo hasta 4 GiB. El pool elimina el pico por encima de lo vivo (+0,35 GiB Chrome, +1,0 GiB Firefox). Planear con que Chrome no devuelve la RAM al soltar. |

## Límites

- Las velocidades son de caché de página, no de disco sostenido; no se midió `fsync` real ni disco lleno, ni una zona que llene la cuota (`QuotaExceededError` con zonas grandes).
- `df` no es utilizable (btrfs lento y máquina compartida); el disco se juzga por `du` del perfil.
- Las liberaciones tras `terminate()` se probaron con 1 y 8 handles en un worker en un bucle síncrono, no con 664 handles ni con una escritura dentro de `flush`; Chrome `file://` no tiene OPFS; Safari y Firefox 158 no se midieron.
- Los 4 GiB de RAM se probaron con el equipo con 20 GB disponibles; no hay datos de máquinas con menos RAM (donde el fallo de reserva podría aparecer antes de 4 GiB).
- La prueba `truncq` se añadió tras la primera pasada completa y se ejecutó sola (3 rondas) con el mismo código del driver.
- Los perfiles y los ficheros grandes de `target/s5-*` se borraron al terminar.
