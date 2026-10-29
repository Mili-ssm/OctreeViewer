# Spike S1: arenas OPFS (29-10-2026)

> Registro fechado. Cifras válidas solo para esta revisión (`74ef00f` para los contadores nativos),
> estos navegadores y este equipo (AMD Ryzen AI 9 365, 30 GB de RAM, NVMe, Linux 7.2.11). Diseño bajo
> prueba: «Spools por arenas de trabajo» en
> [conversor-web-paralelo](../../explanation/conversor-web-paralelo.md).

## Conclusión

| Criterio de parada | Resultado |
| --- | --- |
| Pico modelado > 1,5 × conjunto vivo nativo | **SE CUMPLE en Belvedere (divisiones por defecto, 2,64×) y en Majadas (6,53×)**; no en el modelo optimista de Belvedere con divisiones 8 (1,05× con N=4, 1,15× con N=8); con arenas retenidas hasta la fase de bloques el rango es ~0,94×–1,88× (ver «Cotas con arenas retenidas») |
| OPFS no disponible en `file://` en Firefox | No se cumple: Firefox 157 funciona igual en `file://` y en HTTP |
| OPFS en `file://` en Chrome | **No disponible** (`SecurityError` en el worker, medido en las 6 celdas del `.jsonl`; el hilo principal se comprobó ad hoc, sin fila en el `.jsonl`) |
| Cuota frente a Majadas | **El conjunto vivo nativo de Majadas con el planificador por defecto (19,2 GiB, pico de una sola ejecución con divisiones 2 y 4 workers) supera la cuota medida (10 GiB)**. Depende del planificador (Belvedere: 4,67 GB con 1 worker y 6,0 GB con 4, observados en ejecuciones de desarrollo y no archivados; 3,29 GiB con divisiones 8, archivado; Majadas con divisiones 8 o 1 worker no se midió), así que no se concluye que ningún diseño baste |

### Decisiones para el propietario

1. **`SpoolStore`** (criterio del 1,5× incumplido con divisiones por defecto): replantear antes de la etapa 2.
2. **Conflicto con R8**: Chrome no ofrece OPFS desde `file://` (`SecurityError` en el worker). Si R8
   exige abrir el paquete con doble clic en Chrome, la conversión OPFS no es posible ahí (Firefox sí).
   Hay que decidir si R8 se relaja (servir por HTTP local) o si Chrome queda sin conversión desde `file://`.
3. **Cuota para Majadas**: necesita una cuota por encima de ~19 GiB más la pareja de salida. La cuota
   de un perfil real y `navigator.storage.persist()` no se midieron.

**Hay que replantear `SpoolStore` con el propietario antes de la etapa 2**: con arenas «un fichero por
trabajo de subárbol raíz» el pico OPFS es, con las divisiones por defecto, entre 2,6 y 6,5 veces el
nativo (Majadas llegaría a ~125 GiB). Con el planificador por defecto, el conjunto vivo nativo de Majadas
(19,2 GiB, una ejecución) supera la cuota de 10 GiB medida; con otro reparto podría ser menor (no medido). La medida de OPFS en sí no es el problema (ver más abajo): el
problema es que una arena de solo añadido no libera nada hasta que acaba el trabajo.

---

## Paso 3: modelo

**Fuente:** contadores nativos de
[web-converter-baseline-2026-10-29.json](data/web-converter-baseline-2026-10-29.json)
(`profile_runs`, perfil `perf`, 4 workers, caliente). No se repitieron conversiones.

**Supuestos (explícitos):**

1. Los **spools de nivel 1** son los que la partición de la raíz escribe, uno por hijo raíz
   (`build_root_from_las` → `build_node(.., level 1)` en `spatial.rs`). Su volumen es el de la
   carga útil de la fuente: `spool_bytes_written − Σ root_child_spool_bytes_written`
   (coincide con `block_stream_source_payload_read_bytes`).
2. `root_child_spool_bytes_written[i]` son los bytes que escribe **todo el subárbol del hijo raíz i**
   (sus spools descendientes), sin contar su propio spool de nivel 1. La lista solo recoge hijos
   raíz no vacíos (se descartan los ceros).
3. Un trabajo de subárbol raíz es un trabajo síncrono con una arena de solo añadido: **conserva todos
   los spools que escribe hasta que termina** (no hay borrado ni reutilización dentro de la arena).
4. Los spools de nivel 1 siguen vivos mientras haya subárboles en curso (conservador: no se descuenta
   cada hijo ya consumido).
5. Los N trabajos en curso son los N subárboles mayores (peor caso).
6. Todo va a OPFS (sin RAM). Es un tope: el diseño prevé spools en RAM, pero eso solo mueve el
   problema a la RAM.

**Fórmula:** `pico_modelado = nivel1 + Σ (N mayores root_child_spool_bytes_written)`.
El conjunto vivo nativo es `peak_live_spool_bytes` (exacto salvo búferes de 32 KiB sin volcar) **para una ejecución**: con workers > 1 varía entre ejecuciones por el entrelazado de hilos (Belvedere, 4 workers, divisiones 2: 6,00 frente a 6,13 GB en dos ejecuciones; solo la de 6,13 GB está archivada en el JSON), así que «exacto» no vale para la configuración.

| Conjunto | Hijos raíz no vacíos | Nivel 1 (GiB) | Σ N=4 (GiB) | Σ N=8 (GiB) | Pico vivo nativo (GiB) | Modelo N=4 (GiB) | × nativo | Modelo N=8 (GiB) | × nativo |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Belvedere, divisiones 2 (por defecto) | 4 | 3,12 | 11,94 | 11,94 | 5,71 | 15,06 | **2,64** | 15,06 | **2,64** |
| Belvedere, divisiones 8 | 69 de 81 | 3,12 | 0,34 | 0,66 | 3,29 | 3,46 | 1,05 | 3,78 | 1,15 |
| Majadas, divisiones 2 | 4 de 7 | 12,77 | 112,70 | 112,70 | 19,20 | 125,47 | **6,53** | 125,47 | **6,53** |

Notas:

- Los cocientes de divisiones 2 son exactamente `spool_bytes_written / peak_live_spool_bytes`
  (15,05/5,71 y 125,47/19,20): el caso es degenerado porque N ≥ número de hijos raíz no vacíos, así que
  el modelo equivale a «nada se libera nunca». Sensibilidad: liberar el nivel 1 según se consume cada
  hijo solo baja Belvedere d2 a Σ subárboles / nativo ≈ 11,94/5,71 = **2,09×**, aún > 1,5×.
- El encargo anticipaba 4, 80 y 7 entradas; en el JSON hay 4, 81 (69 no vacías) y 7 (4 no vacías).
- Con 4 hijos raíz, N=8 no añade nada: ya están todos en curso.
- **El 1,05× de divisiones 8 es optimista.** El modelo solo suma los N subárboles en curso, pero según la
  spec («Spools por arenas de trabajo») una arena con spools hoja/LOD vivos no se puede borrar hasta la
  fase de bloques, y una arena de solo añadido no libera nada a mitad de trabajo. Todos los spools de
  subárbol escritos siguen vivos al terminar la fase espacial. Véase «Cotas con arenas retenidas».
- Con divisiones 8 el modelo cabe porque el reparto es fino (máximo 0,087 GiB por subárbol). Con
  divisiones 2 el subárbol mayor de Majadas escribe 42,9 GiB, que el nativo va borrando por el camino y
  una arena de solo añadido acumularía.
- Vía a evaluar con el propietario (no decidida aquí): trabajos más finos que el hijo raíz (por nodo de
  nivel 2), o arenas por generación con borrado intermedio; ambas cambian la partición del paralelismo.
- Incluso en el caso optimista, 3,5–3,8 GiB de pico OPFS caben en la cuota medida (10 GiB), pero 15 GiB y
  125 GiB no.

### Cotas con arenas retenidas (Belvedere, divisiones 8)

Datos del JSON (`profile_runs.belvedere_divisions8`): `spool_bytes_written` = 6.660.894.157 B (6,20 GiB),
carga útil de nivel 1 = `block_stream_source_payload_read_bytes` = 3.348.975.489 B (3,12 GiB), pico vivo
nativo = 3.535.907.518 B (3,29 GiB). Σ subárboles = 6.660.894.157 − 3.348.975.489 = 3.311.918.668 B (3,08 GiB).

| Escenario | Fórmula | GiB | × nativo |
| --- | --- | --- | --- |
| Cota inferior: nivel 1 liberado según se consume; los spools de subárbol quedan vivos hasta la fase de bloques | `Σ subárboles / pico nativo` = 3,08 / 3,29 | 3,08 | **0,94** |
| Cota superior: la arena de la raíz (nivel 1) se retiene hasta el final | `spool_bytes_written / pico nativo` = 6,20 / 3,29 | 6,20 | **1,88** |
| Modelo publicado arriba (N=4) | `nivel 1 + Σ 4 mayores` | 3,46 | 1,05 |

Ambas cotas son aproximaciones de la misma ejecución, no medidas con arenas reales. El 1,05× es
optimista: la cota superior supera el criterio de 1,5×, y la inferior (0,94×) solo lo cumple si el
nivel 1 se libera a medida que se consume. Por tanto, trabajos más finos por sí solos no garantizan
pasar el criterio; hace falta además liberación intermedia (arenas por generación) o spools en RAM.

---

## Paso 4: medida OPFS en navegador

**Método.** Página [opfs-arena-spike-2026-10-29.html](data/opfs-arena-spike-2026-10-29.html) +
[.js](data/opfs-arena-spike-2026-10-29.js): N workers (arrancados desde una URL `Blob`, para que
valga también en `file://`); cada uno crea su arena (`createSyncAccessHandle`), añade **1 GiB** en
bloques de 32 KiB o 1 MiB con `write(..., {at})`, hace `flush()`, la relee entera con `read`
(verificando una marca por bloque; 0 bloques erróneos en todas las ejecuciones), cierra y la borra con
`removeEntry`. MB/s = 10^6 B/s; los agregados son bytes totales de todos los workers entre el tiempo de
la fase del worker más lento.

**Navegadores (headless, perfil persistente nuevo en disco, una ejecución por celda, sin repeticiones):**

- Chrome 156.0.8078.47 (`/usr/bin/google-chrome`) vía Playwright 1.64.1.
- Firefox 157.0 de Playwright (`~/.cache/ms-playwright/firefox-1555`), no el Firefox 158.0.1 del
  sistema (no se pudo gobernar sin el protocolo parcheado de Playwright).
- HTTP: `python3 -m http.server --bind 127.0.0.1 8791` desde la raíz del repositorio; `file://` abriendo
  el HTML directamente. Se probó antes un contexto efímero: la cuota era de ~1,5 GiB en Firefox, así
  que se pasó a perfil persistente (cuota estimada: 10 GiB en Firefox; 11 200–13 248 MiB en Chrome según la fila, con uso inicial no nulo).
- Datos crudos: [opfs-arena-spike-2026-10-29.jsonl](data/opfs-arena-spike-2026-10-29.jsonl).

| Navegador | Origen | N | Bloque | Escritura MB/s | Escritura+flush MB/s | Lectura MB/s | `createSyncAccessHandle` máx. ms | `flush` máx. ms | `removeEntry` máx. ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Chrome 156 | http | 1 | 32 KiB | 5546 | 3362 | 11825 | 1 | 126 | 32 |
| Chrome 156 | http | 1 | 1 MiB | 19523 | 6620 | 17180 | 0 | 107 | 16 |
| Chrome 156 | http | 4 | 32 KiB | 6908 | 5649 | 33140 | 4 | 139 | 75 |
| Chrome 156 | http | 4 | 1 MiB | 10584 | 9246 | 43826 | 0 | 271 | 77 |
| Chrome 156 | http | 8 | 32 KiB | 6995 | 5932 | 14420 | 1 | 230 | 222 |
| Chrome 156 | http | 8 | 1 MiB | 13173 | 9168 | 15252 | 1 | 357 | 142 |
| Chrome 156 | file | 1, 4, 8 | ambos | no medido | no medido | no medido | — | — | — |
| Firefox 157 | http | 1 | 32 KiB | 809 | 737 | 2026 | 9 | 130 | 50 |
| Firefox 157 | http | 1 | 1 MiB | 10737 | 5162 | 13422 | 9 | 108 | 25 |
| Firefox 157 | http | 4 | 32 KiB | 1506 | 1432 | 2622 | 50 | 169 | 101 |
| Firefox 157 | http | 4 | 1 MiB | 15675 | 8642 | 34637 | 49 | 227 | 142 |
| Firefox 157 | http | 8 | 32 KiB | 1133 | 1108 | 1847 | 95 | 206 | 413 |
| Firefox 157 | http | 8 | 1 MiB | 11687 | 8957 | 13443 | 88 | 282 | 197 |
| Firefox 157 | file | 1 | 32 KiB | 808 | 738 | 1928 | 8 | 128 | 48 |
| Firefox 157 | file | 1 | 1 MiB | 10737 | 5187 | 13422 | 12 | 107 | 26 |
| Firefox 157 | file | 4 | 32 KiB | 1624 | 1552 | 2669 | 47 | 161 | 146 |
| Firefox 157 | file | 4 | 1 MiB | 15017 | 8522 | 38009 | 48 | 223 | 161 |
| Firefox 157 | file | 8 | 32 KiB | 1162 | 1146 | 1845 | 96 | 134 | 366 |
| Firefox 157 | file | 8 | 1 MiB | 11468 | 8837 | 12098 | 98 | 260 | 269 |

**Lectura de las cifras.**

- Los rendimientos superan con mucho al NVMe: son de caché de página del SO (el `flush` de 100–350 ms
  por GiB no equivale a un `fsync` completo medido). Valen como **coste de CPU/IPC de la API**, no
  como velocidad de disco sostenida. No se midió con la caché fría ni con ficheros mayores que la RAM.
- **Bloque de 1 MiB frente a 32 KiB:** 1 MiB es de 1,5× a 13× más rápido en escritura, y en Firefox
  la diferencia es de un orden de magnitud (≈0,8–1,5 GB/s con 32 KiB frente a 10–15 GB/s con 1 MiB).
  Recomendación: **añadir en bloques ≥ 1 MiB** (agrupar los búferes de 32 KiB de los hijos antes de
  escribir).
- **Un handle por trabajo es barato:** `createSyncAccessHandle` tarda ≤ 4 ms en Chrome; en Firefox
  ≈ 9 ms con N=1, ≈ 50 ms con N=4 y ≈ 90–100 ms con N=8 (abriendo a la vez). Los 512 ficheros del diseño
  por spool no se midieron; la comparación se basa en el coste por handle, no en una prueba directa.
- `removeEntry` de 1 GiB: 16–413 ms según N; es un coste de fin de trabajo, no del camino caliente.
- Cuota: 10 GiB en Firefox y 11 200–13 248 MiB en Chrome (HTTP), perfiles persistentes nuevos. N=8 × 1 GiB
  cabe, pero los picos modelados de 15 y 125 GiB no cabrían, y el conjunto vivo nativo de Majadas
  (19,2 GiB, planificador por defecto) tampoco.

## Qué no se midió y por qué

| Elemento | Motivo |
| --- | --- |
| OPFS en Chrome desde `file://` | No disponible: `SecurityError` en el worker (medido, 6 celdas del `.jsonl`); hilo principal comprobado ad hoc, sin fila en el `.jsonl` |
| Firefox 158.0.1 del sistema | Solo se puede gobernar con el Firefox parcheado de Playwright (157.0) |
| Safari/WebKit | Fuera de alcance del encargo |
| Caché fría, ficheros mayores que la RAM, repeticiones | Una ejecución por celda; sin varianza |
| Contención con el escritor único de salida y con lectores `getFile()` | No es objeto del spike |
| Patrón de datos | Casi todo ceros (un `setUint32` cada 4096 B): en sistemas de ficheros que comprimen (p. ej. btrfs) infla el rendimiento |
| Patrón de lectura | Solo lectura secuencial del fichero entero; el diseño lee extensiones dispersas |
| Uso inicial de cuota | No nulo en las filas de Chrome (cuota estimada 11 200–13 248 MiB) |
| Cuota efímera de Firefox (~1,5 GiB) | Observada en una prueba previa de 64 MiB, sin fila cruda en el `.jsonl` |
| Firefox 157 | Es la build parcheada de Playwright, no un Firefox de producción |
| Cuota de un perfil de usuario real (porcentaje del disco libre) | Se usó un perfil nuevo de Playwright |

## Para la etapa 2

1. **Replantear el propietario de `SpoolStore`**: el modelo de arena-por-subárbol-raíz no cumple el
   criterio de 1,5× con las divisiones por defecto. Opciones: trabajos más finos o borrado/reciclaje de
   extensiones dentro de una arena.
2. **Conflicto con R8**: `file://` en Firefox funciona; en Chrome no (`SecurityError` en el worker). Si
   el paquete se abre con doble clic en Chrome, la conversión OPFS no estará disponible (el visor puede
   servirse por HTTP local). Decisión del propietario.
3. **Majadas con el planificador por defecto no cabe**: su conjunto vivo nativo (19,2 GiB, una ejecución,
   divisiones 2 y 4 workers) supera los 10 GiB medidos; haría falta una cuota > ~19 GiB más la pareja de
   salida (cuota real y `persist()` no medidos). Con otro reparto (divisiones 8, 1 worker) no se midió.
4. Bloque de añadido ≥ 1 MiB; un handle por trabajo.
