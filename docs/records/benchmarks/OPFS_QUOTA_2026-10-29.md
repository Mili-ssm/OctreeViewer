# S8: cuota real de OPFS por navegador (29-10-2026)

> Registro fechado. Cifras válidas solo para estos navegadores y este equipo (Linux 7.2.11, `/home` en
> btrfs con `compress=zstd:1`, disco de 953 GB). La cuota depende del disco: no se traslada a otros
> equipos. Diseño bajo prueba: [conversor-web-paralelo](../../explanation/conversor-web-paralelo.md)
> (S8; R4). Continúa el hueco «cuota de un perfil real y `persist()` no medidos» de
> [OPFS_ARENA_SPIKE_2026-10-29.md](OPFS_ARENA_SPIKE_2026-10-29.md).

## Conclusión

| Pregunta | Respuesta |
| --- | --- |
| ¿Cabe Majadas (≈ 19 GiB de spools vivos + ≈ 7,5 GB de salida ≈ 27 GB)? | **Sí con `persist()` concedido en Firefox** (cuota declarada 476 GiB, ≥ 40 GiB escritos sin error). **No con la cuota por defecto de Firefox** (10 GiB, o con `persist()` denegado): el error llega a 9,999 GiB. **Chrome: no se encontró el límite**: declara 10 GiB pero aceptó ≥ 40 GiB sin error en los tres estados de `persist()`. |
| ¿Hay que volver al propietario antes de M6? | Sí, por dos puntos (ver «Decisiones»): Firefox sin `persist()` no basta, y la cuota declarada de Chrome no es fiable como preflight (R4). |

**Estimación usada:** la de la spec (≈ 19 GiB de spools vivos + ≈ 7,5 GB de salida ≈ 27 GB; véase
[conversor-web-paralelo](../../explanation/conversor-web-paralelo.md), S8 y M6). El controlador
actualizará la cifra si el modelo de la Tarea 1 difiere; no se recalculó aquí.

### Decisiones para el propietario

1. **Firefox**: la conversión de Majadas exige que `persist()` sea concedido. Con 10 GiB no cabe ni el
   modelo. Hay que decidir si el preflight de R4 rechaza (con mensaje) cuando `persist()` es denegado y la
   estimación supera la cuota.
2. **Chrome**: `estimate().quota` (10 GiB) no coincide con lo escribible (≥ 40 GiB). Un preflight basado
   en `estimate()` rechazaría Majadas en Chrome sin motivo. No se sabe dónde falla Chrome de verdad
   (el tope de seguridad de 40 GiB lo impidió); haría falta una prueba mayor o confiar en el aborto por
   `QuotaExceededError` (R4/R5) en vez de en la estimación.
3. **Firefox y `QuotaExceededError`**: con `FileSystemSyncAccessHandle.write` el fallo **no lanza
   excepción**: `write` devuelve 0 bytes (escritura corta). El aborto limpio de R5 debe tratar una
   escritura corta como error de cuota.

---

## Método

- Página [opfs-quota-2026-10-29.html](data/opfs-quota-2026-10-29.html) + [.js](data/opfs-quota-2026-10-29.js):
  lee `persisted()`, llama (o no) a `persist()`, lee `estimate()`; un worker (desde `Blob`, válido en
  `file://`) añade bloques de 1 MiB con `FileSystemSyncAccessHandle.write` en ficheros de 1 GiB hasta
  `QuotaExceededError`, error de escritura o **tope de seguridad de 40 GiB**; después borra todo
  (`removeEntry` recursivo, también tras un fallo) y vuelve a leer `estimate()`.
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
La fracción es cuota / espacio libre del disco antes de la ejecución (≈ 340–349 GB libres).

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
- Firefox falla en 10 GiB − 1 MiB (10 736 369 664 B): el bloque siguiente de 1 MiB ya no cabe.
- `persist()` en Chrome headless no cambió la cuota declarada ni el comportamiento observado.
- Borrado: `removeEntry` de 40 GiB tardó 2,1–3,1 s en Chrome y 3,0–3,1 s en Firefox; de 10 GiB, 0,6–1,4 s.
  `estimate()` tras limpiar devolvió uso 0 (Chrome) y 491 520 B (Firefox, metadatos).

## Disco y limpieza

- `/home` libre al empezar la tarea: 356 GB; al terminar todas las ejecuciones: 343 GB; las variaciones
  por ejecución (±3 GB, columnas `freeBeforeKB`/`freeAfterKB` del `.jsonl`) vienen sobre todo de otros
  procesos del equipo, no de las escrituras: **los datos son casi todo ceros y btrfs comprime con zstd**,
  así que 40 GiB lógicos casi no ocupan disco. Esta medida es válida para la cuota (que cuenta bytes
  lógicos), no para el consumo físico de disco.
- Ninguna ejecución cayó bajo 100 GB libres ni movió el libre más de un 10 %; todos los perfiles estaban
  en btrfs y se borraron (`profileRemoved` verdadero; `target/q3-profiles` eliminado).

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
