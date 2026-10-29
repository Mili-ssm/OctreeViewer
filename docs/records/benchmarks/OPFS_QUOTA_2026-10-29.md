# S8: cuota real de OPFS por navegador (29-10-2026)

> Registro fechado. Cifras válidas solo para estos navegadores y este equipo (Linux 7.2.11, `/home` en
> btrfs con `compress=zstd:1`, disco de 953 GB). La cuota depende del disco: no se traslada a otros
> equipos. Diseño bajo prueba: [conversor-web-paralelo](../../explanation/conversor-web-paralelo.md)
> (S8). Continúa el hueco «cuota de un perfil real y `persist()` no medidos» de
> [OPFS_ARENA_SPIKE_2026-10-29.md](OPFS_ARENA_SPIKE_2026-10-29.md).

## Conclusión

> Alcance: una ejecución por celda, perfil nuevo de Playwright, `persist()` concedido por preferencias
> de prueba (sin diálogo real), Firefox 157 (no 158), una sola escritura secuencial de un worker (el
> pico concurrente de spools no se probó) y datos de ceros, comprimibles. Ninguna ejecución produjo
> `QuotaExceededError`.

| Pregunta | Respuesta |
| --- | --- |
| ¿Cabe Majadas (≈ 19 GiB de spools vivos + ≈ 7,5 GB de salida ≈ 27 GB)? | **Firefox con `persist()` concedido: no falló hasta 40 GiB en estas condiciones** (cuota declarada 476 GiB). **Firefox con la cuota por defecto o con `persist()` denegado: no cabe**; la escritura se corta (`write` devuelve 0) a 10 GiB − 1 MiB exactos, lo que se infiere que es la cuota de 10 GiB declarada. **Chrome: no falló hasta 40 GiB** en los tres estados de `persist()` (headless, perfil nuevo), por encima de la estimación de Majadas aunque declara 10 GiB; ese valor exacto de 10 GiB queda sin explicar. |
| ¿Hay que volver al propietario antes de M6? | Sí (ver «Decisiones»): Firefox sin `persist()` no basta, y `estimate()` no sirve como preflight de R4 en Chrome. |

**Reconciliación con el modelo de la Tarea 1** ([modelo por nodos](NODE_JOB_ARENA_MODEL_2026-10-29.md)):
solo la variante c (un archivo OPFS por spool de hijo) cumple 1,5 ×. Su pico para Majadas es 2,00 × los
bytes de la fuente (cota superior, ≈ 25,5 GiB); con ≈ 7,5 GB de salida son ≈ 32,5 GiB, por debajo de los
40 GiB probados, así que las respuestas de la tabla valen para c. La variante b (≈ 2,6 × la fuente con
FIFO/LIFO, más con «mayor primero») supera los 40 GiB con la salida y queda sin probar. Firefox con la
cuota por defecto (10 GiB) no admite ninguna variante.

**Estimación usada:** la de la spec (≈ 19 GiB de spools vivos + ≈ 7,5 GB de salida ≈ 27 GB); véase
[conversor-web-paralelo](../../explanation/conversor-web-paralelo.md) (hito M6 y spike S8 en «Hitos» y
«Spikes»).

### Decisiones para el propietario

1. **Firefox**: la conversión de Majadas exige que `persist()` sea concedido. Con 10 GiB no cabe ni el
   modelo. Hay que decidir si el preflight de R4 rechaza (con mensaje) cuando `persist()` es denegado y la
   estimación supera la cuota.
2. **Chrome**: aceptó escrituras por encima de la estimación de Majadas (≥ 40 GiB) en estas condiciones
   (headless, perfil nuevo) con `estimate().quota` = 10 GiB, así que `estimate()` es inutilizable como
   preflight. No se sabe dónde falla Chrome de verdad (el tope de 40 GiB lo impidió) ni por qué declara
   exactamente 10 GiB; haría falta una prueba mayor o confiar en el aborto ante el fallo de escritura
   (R4 y R5 de la [spec](../../explanation/conversor-web-paralelo.md#requisitos)) en vez de en la estimación.
3. **Fallo de cuota en Firefox**: con `FileSystemSyncAccessHandle.write` no se observó
   `QuotaExceededError`: `write` devolvió 0 bytes (escritura corta) a 10 GiB − 1 MiB (que sea la cuota es
   una inferencia). El aborto limpio de R5 debe tratar una
   escritura corta como error de cuota.

---

## Método

- Página [opfs-quota-2026-10-29.html](data/opfs-quota-2026-10-29.html) + [.js](data/opfs-quota-2026-10-29.js):
  lee `persisted()`, llama (o no) a `persist()`, lee `estimate()`; un worker (desde `Blob`, válido en
  `file://`) añade bloques de 1 MiB con `FileSystemSyncAccessHandle.write` en ficheros de 1 GiB hasta
  `QuotaExceededError`, error de escritura o **tope de seguridad de 40 GiB**; después borra todo
  (`removeEntry` recursivo, también tras un fallo) y vuelve a leer `estimate()`.
- Driver: [opfs-quota-2026-10-29-driver.mjs](data/opfs-quota-2026-10-29-driver.mjs), desde la raíz del
  repositorio: `node docs/records/benchmarks/data/opfs-quota-2026-10-29-driver.mjs --out=<fichero>.jsonl`
  (matriz completa) o con celdas `firefox:http:none`. Crea el perfil en `target/q3-profiles`, aplica
  `persist()` (CDP / preferencias), registra `findmnt` y `df`, sirve HTTP y, en `finally`, cierra el
  navegador, mata el servidor y borra el perfil. Los campos `*KiB` del `.jsonl` son KiB y los `*GiB` GiB.
  Las 10 filas del `.jsonl` son anteriores a este driver: salieron de una versión previa (llevan un campo
  `console` que el driver no escribe) y sus `*GiB` están redondeados hacia arriba, mientras que el driver
  usa `floor`; los `*KiB` son la referencia.
- Datos crudos: [opfs-quota-2026-10-29.jsonl](data/opfs-quota-2026-10-29.jsonl) (una fila por ejecución,
  con `df` antes/después, sistema de ficheros del perfil y fracción cuota/disco libre).
- Automatización: Playwright (`playwright-core` 1.64.0-alpha), contexto persistente nuevo por ejecución,
  con el perfil en `target/` del repositorio (btrfs de `/home`, comprobado con `findmnt -T`; `/tmp` es
  tmpfs y no se usó). Perfiles borrados al terminar cada ejecución. Servidor:
  `python3 -m http.server --bind 127.0.0.1` (cerrado al terminar).
- **Chrome 156.0.8078.47** (`/usr/bin/google-chrome`, headless). `persist()` concedido/denegado con
  `Browser.setPermission` de CDP (`persistent-storage`).
- **Firefox 157.0** de Playwright (`~/.cache/ms-playwright/firefox-1555`), no el Firefox 158.0.1 del
  sistema (no se puede gobernar sin el protocolo parcheado, como ya se registró en S1).
  `persist()` concedido/denegado con las preferencias `dom.storageManager.prompt.testing` y
  `dom.storageManager.prompt.testing.allow`; sin ellas `persist()` espera un diálogo y la página se
  cuelga en headless.
- «Ninguna»: `persist()` no se llama (`persisted()` = falso). «Concedido»/«denegado»: se llama y el
  navegador responde `true`/`false`.

## Resultados

Una ejecución por celda, sin repeticiones. Cuota = `estimate().quota` antes de escribir, con uso 0.
La fracción es cuota / espacio libre del disco antes de la ejecución (≈ 340–349 GiB libres).

| Navegador | Origen | `persist()` | `persisted()` tras | Cuota declarada | Cuota / disco libre | Escrito | Fin |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Chrome 156 | http | ninguna | no | 10 GiB | 0,029 | ≥ 40 GiB escritos sin error | tope |
| Chrome 156 | http | concedido | sí | 10 GiB | 0,029 | ≥ 40 GiB escritos sin error | tope |
| Chrome 156 | http | denegado | no | 10 GiB | 0,029 | ≥ 40 GiB escritos sin error | tope |
| Chrome 156 | file | ninguna | no | 10 GiB (hilo principal) | 0,029 | 0 | `SecurityError` en el worker (como en S1) |
| Firefox 157 | http | ninguna | no | 10 GiB | 0,029 | 9,999 GiB | `write` devuelve 0 (escritura corta) |
| Firefox 157 | http | denegado | no | 10 GiB | 0,029 | 9,999 GiB | `write` devuelve 0 |
| Firefox 157 | http | concedido | sí | **476 GiB** | 1,36 | ≥ 40 GiB escritos sin error | tope |
| Firefox 157 | file | ninguna | no | 10 GiB | 0,029 | 9,999 GiB | `write` devuelve 0 |
| Firefox 157 | file | denegado | no | 10 GiB | 0,029 | 9,999 GiB | `write` devuelve 0 |
| Firefox 157 | file | concedido | sí | **476 GiB** | 1,39 | ≥ 40 GiB escritos sin error | tope |

Notas:

- Firefox concedido: 476 GiB es ≈ 50 % de los 953 GB del disco, **mayor que el espacio libre** (fracción
  > 1): la cuota declarada no garantiza espacio real. No se midió escribir hasta llenar el disco (prohibido
  por el tope).
- Firefox falla en 10 GiB − 1 MiB (10 736 369 664 B). Inferencia: 10 GiB − 1 MiB + 1 MiB son exactamente
  10 GiB, así que solo los metadatos (491 520 B tras limpiar) explican que el bloque siguiente de 1 MiB
  ya no quepa.
- `persist()` en Chrome headless no cambió la cuota declarada ni el comportamiento observado.
- Borrado: `removeEntry` de 40 GiB tardó 2,09–2,61 s en Chrome y 3,04–3,13 s en Firefox; de ≈ 10 GiB (Firefox), 0,56–1,43 s.
  `estimate()` tras limpiar devolvió uso 0 (Chrome) y 491 520 B (Firefox, metadatos).

## Disco y limpieza

- Espacio libre de `/home` registrado en el `.jsonl` (`freeBeforeKiB`/`freeAfterKiB`): entre 339,6 y
  349,1 GiB a lo largo de las 10 ejecuciones. El cambio por ejecución fue de −3,1 a +3,0 GiB, en ambos
  sentidos (subió en varias, incluidas ejecuciones de 40 GiB): lo causan otros procesos del equipo, no
  las escrituras. Los datos son casi todo ceros y btrfs comprime con zstd, así que 40 GiB lógicos casi no
  ocupan disco: la medida vale para la cuota (bytes lógicos), no para el consumo físico.
- Ninguna ejecución cayó bajo 100 GiB libres; todos los perfiles estaban en btrfs y se borraron
  (`profileRemoved` verdadero; `target/q3-profiles` vacío al terminar).
- La ejecución de validación del driver (`firefox:http:none`, 9,999 GiB, misma conducta) no se añadió al `.jsonl`.

## Qué no se midió

| Elemento | Motivo |
| --- | --- |
| Límite real de Chrome | El tope de 40 GiB se alcanzó antes de cualquier error |
| Perfil de usuario real (marcadores, engagement, instalado como PWA) | Se usaron perfiles nuevos de Playwright; Chrome puede conceder `persist()` por heurística de uso |
| Firefox 158.0.1 del sistema | No gobernable con Playwright; ver procedimiento manual |
| Cuota con disco casi lleno | No se llenó el disco |
| Escritura concurrente de varios workers contra la cuota | Un solo worker |
| Patrón de datos | Casi ceros (compresión del FS); no mide ancho de banda real |
| Repeticiones | Una ejecución por celda |

## Procedimiento manual (Firefox 158 y perfiles reales)

1. Servir: `python3 -m http.server --bind 127.0.0.1 8791` desde la raíz del repositorio; abrir
   `http://127.0.0.1:8791/docs/records/benchmarks/data/opfs-quota-2026-10-29.html?persist=1&capGiB=40`.
2. En el diálogo de permiso de almacenamiento persistente elegir «Permitir» y repetir con «Bloquear»
   (en un perfil nuevo o tras quitar el permiso del sitio).
3. Copiar el JSON de `#out` (o `window.quotaResult`); `persist=0` omite la llamada.
4. Comprobar `df -h /home` antes y después; si el libre baja de ~100 GB, bajar `capGiB`.
