# S9: un archivo OPFS por spool (29-10-2026)

> Registro fechado. Cifras válidas solo para estos navegadores y este equipo (Linux 7.2.11, `/home` en
> btrfs con `compress=zstd:1`, NVMe de 953 GB). Una ejecución por celda (sin repeticiones): las
> diferencias pequeñas entre filas no son concluyentes. Contexto: decisión del propietario del
> 29-10-2026 de usar la variante c del [modelo por nodos](NODE_JOB_ARENA_MODEL_2026-10-29.md) (un
> archivo por spool de hijo); la arena única de comparación es la del [spike S1](OPFS_ARENA_SPIKE_2026-10-29.md)
> y la cuota la trata [S8](OPFS_QUOTA_2026-10-29.md). Datos crudos:
> [JSONL](data/opfs-spool-files-2026-10-29.jsonl); página y worker:
> [HTML](data/opfs-spool-files-2026-10-29.html), [JS](data/opfs-spool-files-2026-10-29.js);
> [driver](data/opfs-spool-files-2026-10-29-driver.mjs).

## Conclusión

**Criterio de parada activado: hay que volver al propietario antes del paso 3 de la Tarea 4.** Los límites de
cantidad se cumplen en los tres navegadores (ficheros vivos, handles abiertos, sin errores), pero el
rendimiento de extremo a extremo cae más de 2 × frente a la arena única de S1 en todos ellos. Con bloque de
32 KiB (la comparación menos sesgada): 2,2–3,0 × en Chrome y 3,2–5,9 × en Firefox. Con 1 MiB sube a 2,6–3,3 × en
Chrome y 18–33 × en Firefox, pero ese extremo está inflado: S1 escribe ceros y termina 1 GiB por worker en
0,7–2,0 s, mientras S9 escribe ruido y su tiempo lo fija el número de ficheros, no los bytes.
**Atribución:** en Firefox domina el coste por archivo (abrir + cerrar = 65–91 % de N·duración, duración casi
plana entre 32 KiB y 1 MiB y entre N = 4 y 8). En Chrome **no** se puede atribuir el 2,2–3,3 × al número de
ficheros: abrir + cerrar + borrar es solo 5–8 % de N·duración, `flush` y los huecos del camino crítico del
árbol no están atribuidos y la duración no baja de N = 4 a 8. Un control (árbol plano con el mismo número de
ficheros, o un contador de la suma de `flush`) separaría la forma del árbol del coste por archivo; no se hizo.

| Pregunta | Respuesta |
| --- | --- |
| ¿Aguantan ≥ 700 ficheros vivos? | **Sí**: pico de 810–857 en las 24 ejecuciones de S9 (Chrome 156; Firefox 157 por http y por `file://`). |
| ¿Aguantan 8 × 81 handles síncronos abiertos a la vez? | **Sí**: 664 handles simultáneos (8 trabajos anchos de 81 hijos + LOD + entrada) con N = 8 y d8, sin errores. |
| ¿Errores (`NoModificationAllowedError`, cuota, límites)? | **Ninguno** en las 36 ejecuciones (12 por navegador y origen: 4 de S1 y 8 de S9). El pico de bytes vivos fue 2,95 GiB (d2) y 5,37 GiB (d8), bajo los 10 GiB de cuota por defecto de Firefox. |
| ¿Rendimiento frente a la arena única (misma sesión)? | **No cumple el criterio de 2 ×** en extremo a extremo (tabla 1). En Firefox domina un coste fijo por archivo (≈ 26–40 ms de duración por fichero, ≈ 22–25 ms por handle; 3,2–5,9 × con 32 KiB, 18–33 × con 1 MiB por el sesgo de S1) que no depende del tamaño. En Chrome (2,2–3,3 ×; ≈ 2,9–4,4 ms de duración por fichero) la causa no está atribuida a los ficheros (ver arriba). |

> Alcance: ejecución única por celda; Firefox 157 de Playwright (no 158); `persist()` no se tocó; el árbol
> es sintético; los datos son ruido xorshift (incompresible), a diferencia de los ceros de S1. La comparación
> «extremo a extremo» **no es de manzanas con manzanas** (ver «Método»): vale como orden de magnitud, no como razón exacta.
> Sesgos conocidos: (1) S1 escribe ceros y S9 ruido; no se cuantificó cuánto del 1 MiB de Firefox es ese efecto. (2) S9 hace de cada hijo, hoja y LOD un
> fichero propio (9 por trabajo), mientras que la especificación mantiene los spools pequeños como segmentos en RAM con
> volcado a disco: S9 probablemente sobrestima el número de ficheros de la variante c. (3) Escala: Majadas escribe 134,7 GB de spools
> y llega a 27,4 GB de pico en la variante c ([M0](WEB_CONVERTER_BASELINE_2026-10-29.md), [modelo](NODE_JOB_ARENA_MODEL_2026-10-29.md)), muy por encima de los 5,37 GiB de pico de S9. La cuota por defecto de Firefox (10 GiB, según [OPFS_QUOTA](OPFS_QUOTA_2026-10-29.md)) queda por debajo de esos 27,4 GB, así que hace falta `persist()`, como ya concluyó ese registro.

## Método

- **Árbol sintético y cola de listos** (`buildTree` en el JS). N workers `Blob` (4 y 8) toman trabajos de una cola
  FIFO; un trabajo está listo cuando termina su padre. Cada trabajo abre a la vez el fichero de entrada, todos sus
  hijos y un fichero LOD con `createSyncAccessHandle`, lee la entrada, añade a los hijos en rondas de `bloque` bytes
  (32 KiB o 1 MiB) entre todos los handles abiertos, hace `flush` de cada uno y los cierra. Al terminar, el host
  borra con `removeEntry` el fichero de entrada. Las hojas y el LOD se conservan hasta el final.
- **d2**: fan-out 8 en todos los nodos; 101 trabajos, 910 ficheros, 809 retenidos al final.
  **d8**: raíz con 81 hijos; sus 8 mayores hijos son trabajos «anchos» de 81 hijos hoja y el resto tiene fan-out 8;
  24 trabajos, 874 ficheros, 850 retenidos. Un trabajo ancho mantiene 81 hijos + LOD + entrada = 83 handles: 8 a la vez
  dan los 664 medidos (con N = 4: 332).
- **Tamaños.** El JSON de la Tarea 1 solo tiene totales por caso, no tamaños por hijo; los tamaños son una **suposición**:
  pesos lognormales (σ = 1,3) por trabajo, escalados para que todo lo escrito (semilla de la raíz + hijos + LOD)
  sume 7.500 MiB ≈ 7,3 GiB (≤ 8 GiB; los trabajos escriben 4,7–5,9 GiB de eso). Mínimo 4 KiB por fichero. La semilla (fichero raíz) la escribe un worker antes de medir y no entra en las tasas.
- **Datos**: ruido xorshift por worker, con una palabra distinta en cada escritura. **No** son los ceros de S1.
- **Comparación con S1**: misma sesión y navegador se vuelve a ejecutar la página de S1 (N = 4/8, bloque 32 KiB/1 MiB, 1 GiB por worker).
  Dos razones: *e2e* = bytes escritos / duración total (S1: `n·1 GiB / wallMs`, que incluye escritura, `flush`, lectura y borrado; S9: bytes escritos por los trabajos / duración desde el primer trabajo, que incluye aperturas, lecturas, `flush`, cierres, borrados y esperas por las dependencias del árbol);
  *escritura pura* = bytes / tiempo dentro de `write()` (S1: `aggWriteMBps`; S9: tiempo medio por worker). En «escritura pura» S9 **no** es más lento (razón < 1: escribir ficheros pequeños en caché es barato); lo que cuesta está fuera de `write()`.
- **Límite de la comparación e2e**: S1 es un único fichero grande por worker, en paralelo desde el principio; S9 arranca con la raíz sola (un trabajo, N − 1 workers parados) y acaba con trabajos pequeños, así que parte del sobrecoste es la forma del árbol y no el número de archivos. No se separó (haría falta un control con árbol plano y el mismo número de ficheros).
- **Entorno**: Chrome 156.0.8078.47 (`/usr/bin/google-chrome`, headless), Firefox 157.0 de Playwright (`firefox-1555`); http con `python3 -m http.server` en 127.0.0.1; Firefox `file://` con worker desde `Blob` (como S1); perfil persistente nuevo por celda bajo `target/s9-profiles` (btrfs de `/home`, nunca tmpfs), borrado en `finally`. Antes de cada ejecución el driver aborta si `df /home` baja de 100 GiB libres.
- **Pico de ficheros vivos / handles**: contados en el host con los mensajes `opened` y `done` de cada worker (un fichero cuenta como vivo desde que su trabajo lo abre hasta que el host lo borra).

## Resultados

Tabla 1: ficheros vivos, handles abiertos y rendimiento frente a S1 (MB/s = 10⁶ B/s; «S1/S9» > 1 = S9 más lento; en «escritura pura» < 1 = S9 más rápido). Cada fila de S9 se compara con la fila de S1 del mismo navegador, N y bloque.

| Navegador | Esc. | N | Bloque | Pico vivos | Pico handles | Pico vivo (GiB) | e2e S9 (MB/s) | e2e S1 (MB/s) | S1/S9 e2e | S1/S9 escritura pura | Errores |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Chrome 156 http | d2 | 4 | 32 KiB | 810 | 40 | 2,95 | 1.589 | 4.191 | 2,6 × | 0,5 × | 0 |
| Chrome 156 http | d2 | 4 | 1 MiB | 811 | 40 | 2,95 | 2.061 | 6.190 | 3 × | 0,48 × | 0 |
| Chrome 156 http | d2 | 8 | 32 KiB | 813 | 80 | 2,95 | 2.104 | 4.619 | 2,2 × | 0,26 × | 0 |
| Chrome 156 http | d2 | 8 | 1 MiB | 812 | 80 | 2,95 | 2.091 | 5.363 | 2,6 × | 0,12 × | 0 |
| Chrome 156 http | d8 | 4 | 32 KiB | 852 | 332 | 5,37 | 1.649 | 4.191 | 2,5 × | 0,41 × | 0 |
| Chrome 156 http | d8 | 4 | 1 MiB | 853 | 332 | 5,37 | 1.867 | 6.190 | 3,3 × | 0,53 × | 0 |
| Chrome 156 http | d8 | 8 | 32 KiB | 851 | 664 | 5,37 | 1.547 | 4.619 | 3 × | 0,31 × | 0 |
| Chrome 156 http | d8 | 8 | 1 MiB | 857 | 664 | 5,37 | 2.010 | 5.363 | 2,7 × | 0,12 × | 0 |
| Firefox 157 http | d2 | 4 | 32 KiB | 811 | 40 | 2,95 | 210 | 1.041 | 5 × | 0,63 × | 0 |
| Firefox 157 http | d2 | 4 | 1 MiB | 811 | 40 | 2,95 | 174 | 5.276 | 30,3 × | 0,47 × | 0 |
| Firefox 157 http | d2 | 8 | 32 KiB | 814 | 80 | 2,95 | 200 | 716 | 3,6 × | 0,31 × | 0 |
| Firefox 157 http | d2 | 8 | 1 MiB | 813 | 80 | 2,95 | 243 | 4.310 | 17,8 × | 0,15 × | 0 |
| Firefox 157 http | d8 | 4 | 32 KiB | 853 | 332 | 5,37 | 176 | 1.041 | 5,9 × | 0,64 × | 0 |
| Firefox 157 http | d8 | 4 | 1 MiB | 853 | 332 | 5,37 | 219 | 5.276 | 24 × | 0,46 × | 0 |
| Firefox 157 http | d8 | 8 | 32 KiB | 853 | 664 | 5,37 | 182 | 716 | 3,9 × | 0,32 × | 0 |
| Firefox 157 http | d8 | 8 | 1 MiB | 857 | 664 | 5,37 | 214 | 4.310 | 20,1 × | 0,2 × | 0 |
| Firefox 157 file | d2 | 4 | 32 KiB | 811 | 40 | 2,95 | 216 | 856 | 4 × | 0,56 × | 0 |
| Firefox 157 file | d2 | 4 | 1 MiB | 811 | 40 | 2,95 | 240 | 4.937 | 20,6 × | 0,45 × | 0 |
| Firefox 157 file | d2 | 8 | 32 KiB | 814 | 80 | 2,95 | 221 | 713 | 3,2 × | 0,24 × | 0 |
| Firefox 157 file | d2 | 8 | 1 MiB | 814 | 80 | 2,95 | 245 | 4.985 | 20,3 × | 0,19 × | 0 |
| Firefox 157 file | d8 | 4 | 32 KiB | 853 | 332 | 5,37 | 192 | 856 | 4,5 × | 0,44 × | 0 |
| Firefox 157 file | d8 | 4 | 1 MiB | 853 | 332 | 5,37 | 148 | 4.937 | 33,4 × | 0,51 × | 0 |
| Firefox 157 file | d8 | 8 | 32 KiB | 855 | 664 | 5,37 | 170 | 713 | 4,2 × | 0,32 × | 0 |
| Firefox 157 file | d8 | 8 | 1 MiB | 857 | 664 | 5,37 | 204 | 4.985 | 24,5 × | 0,19 × | 0 |


Tabla 2: latencias por operación en ms (p50 / p95 / máx) y tiempo acumulado de apertura y cierre de todos los handles de cada trabajo, sumado sobre todos los trabajos (segundos de worker, repartidos entre N workers).

| Navegador | Esc. | N | Bloque | getFileHandle | createSyncAccessHandle | flush | removeEntry | Σ apertura / cierre por trabajo (s) |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Chrome 156 http | d2 | 4 | 32 KiB | 0,3 / 0,8 / 3,9 | 0,4 / 1,1 / 6 | 4,7 / 26,7 / 516,7 | 1,3 / 6,3 / 30,8 | 0,8 / 0,2 |
| Chrome 156 http | d2 | 4 | 1 MiB | 0,2 / 0,5 / 1,2 | 0,3 / 0,5 / 1,4 | 4,5 / 26,2 / 558,4 | 0,5 / 1,8 / 9,5 | 0,6 / 0,2 |
| Chrome 156 http | d2 | 8 | 32 KiB | 0,3 / 1,5 / 19,9 | 0,4 / 1,3 / 11,2 | 8,5 / 41,6 / 473,3 | 1,4 / 8,2 / 41,1 | 1,1 / 0,4 |
| Chrome 156 http | d2 | 8 | 1 MiB | 0,4 / 0,8 / 2,3 | 0,4 / 0,9 / 2,2 | 8,2 / 48,8 / 686,6 | 0,7 / 2,2 / 8,4 | 0,9 / 0,3 |
| Chrome 156 http | d8 | 4 | 32 KiB | 0,2 / 0,4 / 10,5 | 0,2 / 0,5 / 7,5 | 4,1 / 14,1 / 668,2 | 2 / 7,6 / 48,9 | 0,4 / 0,1 |
| Chrome 156 http | d8 | 4 | 1 MiB | 0,3 / 0,4 / 1,6 | 0,3 / 0,5 / 2,8 | 3,9 / 17 / 734 | 0,8 / 2,6 / 15,4 | 0,6 / 0,1 |
| Chrome 156 http | d8 | 8 | 32 KiB | 0,4 / 0,6 / 10,5 | 0,5 / 0,7 / 10,7 | 6,7 / 21,6 / 590,1 | 3,8 / 12,9 / 57,6 | 0,9 / 0,3 |
| Chrome 156 http | d8 | 8 | 1 MiB | 0,4 / 0,5 / 9,9 | 0,4 / 0,6 / 9,9 | 6,4 / 27,4 / 725,1 | 0,8 / 2,9 / 10,9 | 0,7 / 0,2 |
| Firefox 157 http | d2 | 4 | 32 KiB | 15 / 29 / 165 | 34 / 49 / 205 | 3 / 12 / 236 | 27 / 42 / 51 | 50,4 / 36,9 |
| Firefox 157 http | d2 | 4 | 1 MiB | 15 / 33 / 1.170 | 34 / 59 / 2.774 | 3 / 28 / 808 | 28 / 117 / 1.168 | 72,7 / 53,2 |
| Firefox 157 http | d2 | 8 | 32 KiB | 27 / 63 / 305 | 72 / 190 / 352 | 3 / 14 / 521 | 37 / 69 / 199 | 111,3 / 90,1 |
| Firefox 157 http | d2 | 8 | 1 MiB | 27 / 51 / 79 | 81 / 103 / 343 | 3 / 26 / 444 | 38 / 62 / 75 | 107,9 / 76,5 |
| Firefox 157 http | d8 | 4 | 32 KiB | 14 / 27 / 52 | 36 / 49 / 104 | 3 / 13 / 178 | 28 / 51 / 70 | 44,1 / 28,7 |
| Firefox 157 http | d8 | 4 | 1 MiB | 17 / 24 / 48 | 36 / 51 / 90 | 3 / 13 / 809 | 26 / 46 / 47 | 45,3 / 29,5 |
| Firefox 157 http | d8 | 8 | 32 KiB | 20 / 38 / 71 | 79 / 100 / 155 | 3 / 10 / 178 | 35 / 60 / 81 | 84,1 / 56,1 |
| Firefox 157 http | d8 | 8 | 1 MiB | 23 / 41 / 64 | 85 / 111 / 165 | 6 / 23 / 821 | 39 / 67 / 86 | 92,8 / 57,5 |
| Firefox 157 file | d2 | 4 | 32 KiB | 14 / 28 / 117 | 32 / 48 / 307 | 3 / 13 / 516 | 25 / 39 / 287 | 49,6 / 35,5 |
| Firefox 157 file | d2 | 4 | 1 MiB | 15 / 28 / 49 | 37 / 54 / 110 | 3 / 20 / 511 | 27 / 40 / 66 | 53,8 / 37,3 |
| Firefox 157 file | d2 | 8 | 32 KiB | 24 / 49 / 219 | 70 / 100 / 342 | 3 / 18 / 562 | 37 / 57 / 293 | 98,2 / 79,6 |
| Firefox 157 file | d2 | 8 | 1 MiB | 28 / 51 / 100 | 78 / 112 / 197 | 3 / 21 / 513 | 34 / 53 / 67 | 107,9 / 75,1 |
| Firefox 157 file | d8 | 4 | 32 KiB | 12 / 23 / 42 | 39 / 53 / 77 | 3 / 11 / 183 | 26 / 61 / 64 | 44,2 / 29,9 |
| Firefox 157 file | d8 | 4 | 1 MiB | 15 / 59 / 162 | 50 / 159 / 361 | 4 / 14 / 834 | 28 / 157 / 165 | 79,9 / 37,3 |
| Firefox 157 file | d8 | 8 | 32 KiB | 22 / 46 / 118 | 87 / 129 / 224 | 4 / 14 / 522 | 42 / 88 / 91 | 95,3 / 58,8 |
| Firefox 157 file | d8 | 8 | 1 MiB | 23 / 46 / 146 | 92 / 128 / 386 | 5 / 24 / 859 | 45 / 65 / 66 | 102,3 / 59,1 |



## Lectura

- **Cantidad de ficheros y handles: sin problema en esta máquina.** 810–857 ficheros vivos y 664 handles síncronos abiertos a la vez no producen ningún error en Chrome ni en Firefox; `createSyncAccessHandle` sobre un fichero cerrado por su escritor funcionó siempre (sin `NoModificationAllowedError`).
- **Chrome**: `getFileHandle`, `createSyncAccessHandle` y `removeEntry` cuestan ≈ 0,2–4 ms (p50 < 4 ms); `flush` es lo más caro (p50 4–9 ms, máx hasta 0,7 s). El e2e es 1,5–2,1 GB/s frente a 4–6 GB/s de S1: 2,2–3,3 ×.
- **Firefox**: `getFileHandle` p50 12–28 ms, `createSyncAccessHandle` p50 32–92 ms, `removeEntry` p50 20–45 ms y el cierre de los handles ≈ 40–100 ms por fichero; crece con N (los 8 workers se serializan). Resultado: ≈ 150–250 MB/s de e2e **casi independiente del bloque**, es decir, el tiempo lo fija el número de ficheros (≈ 910 por ejecución), no los bytes. Frente a S1 con bloque de 1 MiB (≈ 4–5 GB/s) la razón sube a 18–33 ×; con 32 KiB, donde S1 ya es lento en Firefox (0,7–1,0 GB/s), es 3,2–5,9 ×.
- `file://` en Firefox no difiere de http dentro del ruido de una ejecución por celda.
- **Coste por fichero (tiempo de duración, no de worker).** Los 40–100 ms de apertura/cierre de arriba son tiempo de worker bajo contención (se duplican de N = 4 a 8). Dividiendo la duración de la ejecución entre ficheros (874–910): Firefox ≈ 26–40 ms por fichero (≈ 22–25 ms por handle), Chrome ≈ 2,9–4,4 ms por fichero.
- **Implicación para la Tarea 4** (decisión del propietario): el coste por fichero es el factor a vigilar en Firefox. No se probaron mitigaciones (reutilizar ficheros, agrupar hijos pequeños en una arena, abrir handles bajo demanda).
- **Proyección a Majadas (inferencia, no medida).** Con 1,3 k–7,5 k ficheros a los costes por fichero anteriores, el sobrecoste sería de ≈ +30–300 s en Firefox y ≈ +4–30 s en Chrome, frente a los 221,96 s del CLI nativo ([M0](WEB_CONVERTER_BASELINE_2026-10-29.md)). Supone que el coste por fichero no cambia con el tamaño ni con el número de ficheros vivos, y los sesgos (1) y (2) de la nota de alcance van en sentidos opuestos.

## Disco y limpieza

- `df /home` libre antes y después de la tanda: 351 GiB y 351 GiB (347–350 GiB entre celdas mientras btrfs libera). Perfiles de `target/s9-profiles` borrados (`profileRemoved: true` en las tres celdas); servidor http terminado; cada página borra su directorio OPFS (`cleanupMs` en el JSONL).
- Tras una primera pasada completa se añadieron a la página `getFileHandle` y las sumas de apertura/cierre/borrado (la pasada dio ficheros vivos, handles y rendimiento equivalentes); el JSONL entregado es la segunda pasada completa, con el mismo código para todas las filas.

## Qué no se midió

- Una sola repetición por celda; sin intervalo de confianza.
- Safari/WebKit; Firefox 158; perfiles reales con `persist()` concedido; cuota de Firefox superior a 10 GiB.
- Tamaños reales de spool por hijo (suposición lognormal), Majadas completa (el árbol sintético tiene 874–910 ficheros, no los 642 vivos del modelo) y la lógica real del conversor.
- Mitigaciones del coste por fichero en Firefox.
