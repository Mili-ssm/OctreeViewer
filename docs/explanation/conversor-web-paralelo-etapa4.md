# Conversor web paralelo — plan de la etapa 4

> **Para agentes:** sub-skill obligatoria: superpowers:subagent-driven-development (recomendada) o
> superpowers:executing-plans, tarea a tarea. Los pasos usan casillas (`- [ ]`).

**Estado (30-10-2026): Etapa 4 cerrada.** Resultado: Belvedere en Chrome y Firefox con SHA = R1 a 2,1–2,9 × nativo, ADR 0017 aceptado y conversor antiguo retirado; W1 resuelta (30-10-2026): fusión en `master` tras `scripts/check.sh` (pendiente de ejecutar; [requisitos](../requirements.md#resoluciones-del-30-10-2026)). Detalle por tarea:

Tareas 1 y 2 hechas. Zonas OPFS viables en Chrome y Firefox, sin criterio de parada ([registro](../records/benchmarks/OPFS_ZONES_2026-10-30.md)): `at` > 2³² correcto, escritura de 256 KiB ≥ 3,6 GB/s en Firefox; Chrome solo libera cuota al cerrar el handle y tarda ~2 s en liberar un handle tras `terminate()`. Política y granularidad ([registro](../records/benchmarks/HYBRID_POLICY_2026-10-30.md)): segmentos progresivos recortados al cerrar, lectores fijados sin romper el presupuesto, páginas de 1 MiB a 1,98 × el pico vivo volcado frente a ≤ 1,18 × con 256 KiB. **Decisiones del coordinador:** páginas de 256 KiB, zonas de 1 GiB, segmentos de JS en clases de tamaño reutilizadas, orden de volcado P3 con dos pistas del planificador. Tarea 3 hecha (firmas abajo). Tarea 4 hecha: pistas del planificador (`c704de1`, SHA de R1 intactos), almacén híbrido (`767c811`), volcado a zonas paginadas (`91e73c3`; mismos bytes que en memoria con presupuestos de 64 KiB y 0), estimación (`819adcc`; 13/10/49 zonas como el modelo), arnés nativo C6 `native_hybrid` (`5a39ca0`) y modo solo RAM sin zonas (`3155d5c`). Tarea 5a hecha: tiers JS y bindings reales con paridad en Node y `QuotaExceededError` (`check.sh`, 23 pasos). Tareas 5b–5d hechas; Tarea 9 (código) hecha. Puerta 5e y R7 corto de 4a pasados. Tarea 6 hecha (`bdf8f8e`, [registro](../records/benchmarks/M3B_BELVEDERE_2026-10-30.md)): Chrome y Firefox convierten Belvedere (divisiones 2 y 8; HTTP y Firefox `file://`) con SHA = R1 en las 8 celdas, a 2,13–2,93 × `native_hybrid` (parada en 4 ×), PSS 3,4–4,5 GiB con 2 GiB de spools y sin zonas sobrantes; Chrome `file://` convierte el LAS pequeño solo en RAM y rechaza Belvedere antes de empezar. **Decisión 3 resuelta con los datos:** 2 GiB (mismo tiempo que 4 GiB dentro de la dispersión, 2,3 GiB menos de PSS) queda por defecto. Tareas 7 y 9 verificadas en Chrome y Firefox (Playwright): validación de opciones, teclado, conversión con SHA = R1, `formatVersion` v7.3-pre, error de presupuesto idéntico al CLI, cancelación de Belvedere en jerarquía y en escritura sin parciales, 390 px sin desbordamiento, consola limpia; la verificación encontró que `convert()` descartaba las estadísticas de spools y un prefijo de error duplicado (corregidos en `94600e4`). Tarea 8 hecha (`0cef782`: `browser_convert.rs` y sus módulos exclusivos eliminados; `engine: "core"` obsoleto y `"legacy"` con error, D4). Tarea 10 hecha.

**Objetivo:** cerrar M3b: un worker convierte Belvedere en el navegador con spools fuera de la
memoria wasm (RAM de JS más zonas OPFS), con paridad byte a byte con el CLI en Chrome y Firefox,
tiempo dentro de 4 × el mismo código nativo, picos de RAM y OPFS medidos con presupuestos de 2 y
4 GiB, y el conversor antiguo (`browser_convert.rs`) retirado.

**Arquitectura:** la [spec](conversor-web-paralelo.md) es la autoridad; la etapa anterior es la
[etapa 3](conversor-web-paralelo-etapa3.md). M3a guarda todos los spools en la memoria wasm
(`MemSpoolStore`, `crates/viewer_wasm/src/core_convert.rs:218`) y el pico modelado con un worker
(4,35 GiB en divisiones 2, [S10](../records/benchmarks/HYBRID_SPOOL_RAM_2026-10-29.md)) supera el
techo de 4 GiB de wasm32, así que M3b saca los spools de wasm. `convert_serial` sigue siendo una
sola llamada síncrona: un `HybridSpoolStore` del núcleo guarda los spools en segmentos de JS y, por
encima de un presupuesto, los vuelca de forma síncrona a páginas de zonas OPFS cuyos handles abre
el worker antes de la llamada. R1 se cumple por construcción (mismo planificador y orden de commit).

**Decisión del propietario 5 (30-10-2026)**, tras el análisis de opciones del arquitecto:

| Id | Decisión | Alternativas descartadas |
| --- | --- | --- |
| D1 | **Zonas OPFS paginadas** con handles síncronos abiertos antes de convertir; páginas reutilizables y zonas vacías truncadas. Enmienda las decisiones 2 y 3 («un fichero OPFS por spool volcado»); ADR 0017 | P3 literal con trabajos por pasos (esfuerzo L ahora; Firefox paga 22–40 ms por fichero); pool de handles por spool (542–1 246 handles en Belvedere, ~6 000 en Majadas; S9 probó 664) |
| D2 | La parada de 4 × se mide contra **`convert_serial` nativo** (mismo código, un hilo), en la misma sesión; el CLI por defecto se informa como contexto | CLI con 1 worker espacial; CLI por defecto |
| D3 | Chrome `file://` (sin OPFS): **rechazo temprano** si la estimación de spools + salida en RAM supera `ram_budget`, con mensaje que sugiere servir por HTTP | Intentar y abortar |
| D4 | `engine: "core"` se **acepta sin efecto** y se marca obsoleta; `"legacy"` da un error claro | Eliminar la opción |

Decisión técnica del coordinador: el brazo de 4 GiB de spools se mide con `ram_budget` = 8 GB
(máximo de R9), porque 4 GiB de spools más wasm y la base del navegador superan el defecto de 4 GB.

**Tecnología:** Rust nightly fijado, `viewer_convert_core`, wasm-bindgen 0.2.129,
`FileSystemSyncAccessHandle` (`read`/`write` con `at`, `truncate`, `flush`, `close`), Node 22
(`node:test`), Playwright 1.64 vía scripts Node (como los drivers de S8/S9/M3a).

**Spec:** [conversor-web-paralelo.md](conversor-web-paralelo.md).

## Restricciones globales

- `scripts/check.sh` en verde (todos los pasos) al cerrar cada tarea.
- No cambiar la fecha del nightly ni añadir `#![feature]`.
- `FormatVersion::V<n>` solo en `crates/viewer_format`.
- R1: SHA de [referencia R1](../records/benchmarks/R1_REFERENCE_SHA_2026-10-29.md) (LAS pequeño `176ee42d…`, Belvedere `ecdc4889…`, Belvedere divisiones 8 `3aee2890…`, Majadas `38168777…`).
- R7 solo si una tarea toca la ruta nativa del CLI: mediana pareada de `real_dataset_bench convert` en Belvedere (4 workers, perfil `perf`), 7–10 pares A/B en máquina en reposo, ±3% frente a la base de la misma sesión.
- El núcleo sigue sin E/S, hilos, reloj ni funciones de libm (lint de clippy, ADR 0014 y 0016); sin `unsafe` nuevo.
- R8: funciona con `python3 -m http.server` y desde el paquete `file://` sin COOP/COEP; sin `SharedArrayBuffer`.
- Registros nuevos fechados con el día de ejecución, en español; no editar registros ya mergeados.
- Commits convencionales en inglés con la línea `Co-Authored-By` del modelo que los escribe; sin push.
- Tareas en paralelo con código: worktree y `CARGO_TARGET_DIR` propios; mediciones de tiempo sin otros agentes activos.

## Review Focus

1. **Volcar un spool aún abierto**: los `append` posteriores van a páginas de la zona y la lectura devuelve los mismos bytes que si nunca se hubiera volcado (Tarea 4, test de propiedad con presupuesto diminuto).
2. **Zona o cuota llena**: una escritura corta en OPFS aborta con un error de cuota claro y limpia parciales y zonas (R4, R5) (Tareas 4 y 5).
3. **Miles de spools pequeños** (divisiones 8, LOD y hojas retenidos hasta la fase de bloques): la granularidad de los segmentos no multiplica la RAM; los segmentos crecen de forma progresiva (Tareas 2 y 3).
4. **Cancelar durante la escritura de un dataset grande**: tras `terminate()` los parciales y zonas se borran o se limpian en el siguiente arranque, y una reconversión al mismo nombre funciona (Tarea 7).
5. **Chrome `file://`**: el LAS pequeño se convierte solo en RAM con aviso; Belvedere se rechaza antes de empezar con mensaje claro (D3) (Tareas 5 y 6).

## Enmiendas del pre-mortem (30-10-2026)

Ya aplicadas en el texto de las tareas; se listan para trazabilidad.

| # | Enmienda | Tareas |
| --- | --- | --- |
| C1 | Nunca se vuelca un spool con lector vivo (la entrada del trabajo en curso y lo que lee la fase de bloques); test de propiedad que vuelca en cada `append` mientras se particiona la entrada | 2, 3, 4 |
| C2 | La Tarea 5 se divide: tiers del host como módulo JS con tests Node sobre handles falsos respaldados por `fs` (`at` > 2³², `write` corto); cableado del worker; salida en RAM de Chrome `file://`; preflight R4 y limpieza | 5 |
| C3 | Nueva puerta antes del navegador: Belvedere en Node a través de los bindings de `viewer_wasm` (shim de `FileReaderSync`, handles sobre `fs`): SHA = R1, `memory.buffer.byteLength` y tiempo frente al nativo | 5e |
| C4 | Asignación por páginas con desperdicio de cola y política consciente del tamaño (no volcar o empaquetar spools menores que una página); páginas de 64 KiB, 256 KiB y 1 MiB | 1, 2 |
| C5 | Zonas holgadas: Z = ⌈2 × pico modelado / zona⌉ + 1; test de agotamiento de zonas con error claro | 2, 3, 4 |
| C6 | Arnés nativo de la parada de 4 × = `convert_serial` sobre el mismo `HybridSpoolStore` con páginas en fichero y el mismo presupuesto; también el tiempo con `MemSpoolStore` | 3, 6 |
| C7 | Los `append` a un spool abierto ya volcado se acumulan en una cola de 1 MiB; la Tarea 1 mide `write({at})` de 32–256 KiB | 1, 3 |
| C8 | Segmentos reutilizados por clases de tamaño; bytes por import tipado de wasm-bindgen (`&[u8]`, vista por llamada, nunca retenida), no `Reflect` + `Uint8Array` nuevo; la Tarea 1 mide el churn | 1, 3 |
| C9 | Lecturas de un tier ≤ `SPOOL_IO_BUFFER_BYTES` (test); la Tarea 6 registra la memoria wasm | 4, 6 |
| C10 | La Tarea 8 (retirar el motor antiguo) solo tras pasar las Tareas 6 y 7 en ambos navegadores; si fallan, el antiguo queda como alternativa optativa | 8 |
| C11 | `persist()` y comprobación de cuota de Firefox antes de crear zonas; la limpieza de huérfanos se espera antes de la primera conversión; escritura corta de la salida → mismo error de cuota | 5 |
| C12 | Tiempos de la Tarea 1 sin otros agentes pesados | 1, 2 |
| C13 | Si la Tarea 2 pide una pista del planificador, va en una tarea propia con puertas R1 y R7 | 3 |

**Criterios de parada añadidos:** lectura/escritura más allá de 2³² falla o `truncate` no libera
uso → zonas < 4 GiB y volver al propietario con D1; Firefox escribe datos incompresibles a
< 100 MB/s o un handle tarda > 5 s en liberarse tras `terminate()` → replantear presupuesto y
limpieza antes de la Tarea 3; Belvedere en Node con SHA ≠ R1 → parar y bisecar antes del
navegador; cociente wasm/nativo en Node > 3 × → revisión del propietario; memoria wasm > 1 GiB o
PSS > `ram_budget` + 1 GiB → parar y localizar qué sigue en wasm; máximo de OPFS > 1,5 × el pico de
bytes volcados vivos → replantear página y política; Tarea 6 > 4 × en algún navegador → replantear
según la spec y no ejecutar la Tarea 8.

---

### Tarea 1: spike S5 residual — zonas OPFS

**Pregunta:** ¿soporta OPFS lo que exigen las zonas? Un fichero > 4 GiB con `write`/`read({at})`
más allá de 2³², `truncate` que libera cuota de verdad, ancho de banda con datos incompresibles y
tiempo hasta liberar el bloqueo de un handle tras `terminate()`. También: PSS de 4 GiB de
`ArrayBuffer` en un worker (S10 midió hasta 2 GiB).

**Archivos:** `docs/records/benchmarks/OPFS_ZONES_<aaaa-mm-dd>.md` y `docs/records/benchmarks/data/opfs-zones-<aaaa-mm-dd>.{html,js,-driver.mjs,jsonl}`.

- [x] **Paso 1:** driver Playwright (convenciones de S9) en Chrome y Firefox por HTTP y Firefox `file://`: fichero de 6 GiB con ruido, lecturas en `at` > 4 GiB verificadas, `truncate(0)` frente a `estimate().usage` y `df`, MB/s de escritura y lectura con páginas de 1 MiB, tiempo de liberación del handle tras `terminate()` y PSS de 4 GiB de segmentos (32 KiB y 1 MiB).
- [x] **Paso 2:** registro con decisión (tamaño de zona y de página, reintentos de limpieza) y commit.

### Tarea 2: modelo de la política de volcado y de la granularidad

**Archivos:** `crates/viewer_convert/benchmarks/model_hybrid_spools.py` y su test.

- [x] **Paso 1:** tests con el árbol manual de 5 nodos para la política «cerrados más antiguos primero, abiertos al final», sin pistas del núcleo.
- [x] **Paso 2:** sobre las trazas de S10 (Belvedere divisiones 2 y 8, Majadas), N = 1, presupuestos de 2 y 4 GiB: bytes volcados, máximo de páginas (high-water mark), relecturas desde OPFS (thrashing) frente al orden P3 de S10, y RAM desperdiciada con segmentos fijos de 1 MiB frente a segmentos progresivos (p. ej. 32 KiB → 1 MiB). Decide si hace falta una pista del planificador. Registro nuevo `docs/records/benchmarks/HYBRID_POLICY_<aaaa-mm-dd>.md` (el de S10 no se edita); commit.

### Tarea 3: firmas de M3b (arquitecto)

Con los resultados de las Tareas 1 y 2, un arquitecto (Opus) fija en una sección «Firmas fijadas»
de este documento: `RamTier`/`PageTier`/`HybridSpoolStore` en el núcleo (puro, `Sync` sin `unsafe`:
estado Rust con claves `u32`, objetos JS en un objeto host del worker), tamaño y crecimiento de
segmentos, páginas y zonas, dimensionado de Z a partir de la estimación, política de volcado,
lector en streaming (sin copiar el extent entero), reglas de error (`StorageFull`/sin memoria con
variantes existentes y su `Display`), estimación del rechazo temprano de Chrome `file://` (D3),
lugar de `OpfsWriter`, arnés nativo de la parada de 4 × (D2, C6) y forma de retirar el motor antiguo
(D4). Incluye: fijación de spools con lector vivo (C1), Z holgado (C5), cola de 1 MiB para spools
abiertos volcados (C7), reutilización de segmentos por clases y paso de bytes por import tipado (C8)
y, si la Tarea 2 pide pista del planificador, una tarea propia (C13).

- [x] **Paso 1:** firmas anotadas aquí; revisión del coordinador (aceptadas el 30-10-2026 sin cambios).

#### Firmas fijadas en la Tarea 3 (30-10-2026)

Resumen vinculante (el detalle completo, con firmas Rust/JS, está en el informe del arquitecto
recogido en este commit; las Tareas 4 y 5 siguen el desglose de la tabla de abajo):

- **Núcleo `crates/viewer_convert_core/src/pipeline/hybrid/`** (`tiers.rs`, `pages.rs`, `store.rs`, `estimate.rs`): `RamTier`/`PageTier` con claves `u32` (sin política); `HybridSpoolStore<R, P>` con política pura; `HybridReader` en streaming (≤ 64 KiB por lectura, C9) que **ancla** el spool mientras vive (C1, guard con `Drop`; `delete` con lector vivo es error); `VecRamTier`/`VecPageTier` para tests y el arnés C6.
- **Constantes:** páginas de 256 KiB, zonas de 1 GiB, segmentos de 32 KiB que duplican hasta 1 MiB y se recortan al cerrar a múltiplos de 32 KiB; el presupuesto compara **capacidad** retenida.
- **Volcado (P3 con pistas):** víctimas por orden: cerrados `retained` (mayor primero) → cerrados con rango (mayor rango primero) → cerrados sin pista (más antiguo primero) → abiertos (mayor primero); nunca anclados; presupuesto blando con `over_budget_peak_bytes`. Cola de un spool abierto volcado: **una página** (C7 pasa de 1 MiB a 256 KiB). Páginas libres reutilizadas por el **índice más bajo**; a mitad de conversión solo `truncate(0)` de zonas vacías.
- **Errores** sin variante nueva (`ConvertError::Io`): `StorageFull` (escritura corta o cuota), `Other` («spill zones exhausted…»), `OutOfMemory` (segmento de JS).
- **Pistas (Tarea 4a, C13):** `SpoolStore::hint_retained` y `hint_consume_rank` con cuerpo vacío por defecto; llamadas en `spatial.rs` (`process_child_tasks`, `commit_node`); exigen R1 (4 SHA) y R7 corto (5 pares).
- **Estimación (`estimate.rs`, aritmética entera):** `SpoolEstimate` (vivo, nodos, volcado, zonas, salida, `ram_only`) con las fórmulas de la Tarea 2; Z = ⌈2·volcado/zona⌉ + 1.
- **wasm:** `spool_host.rs` (import estructural `SpoolHost` con `&[u8]`/`&mut [u8]` por llamada, sin snippets) y `opfs.rs` (`OpfsWriter`/`OpfsReader` movidos; escritura corta → `StorageFull`); `JsValue` es `Send + Sync` sin `atomics`, con aserción de compilación; opción `spoolBudgetBytes` (2 GiB por defecto) fuera de `ConvertOptions`.
- **Host:** `examples/web-host/spool-tiers.mjs` (`createSpoolHost`, `createMemorySyncHandle`, `removeWithRetry` cada 50 ms hasta 10 s) y falso de Node `tests/support/fs-sync-handle.mjs`.
- **Arneses:** C6 `tools/convert_parity/src/bin/native_hybrid.rs` (perfil `release` en ambos lados); C3 `tools/convert_parity/run-bindings.mjs` (target `web` + `initSync`, shims de `FileReaderSync` y `Blob`).

| Tarea | Archivos | Aceptación |
| --- | --- | --- |
| 4a pistas | `spool_store.rs`, `spatial.rs` | `retained` una vez por spool de bloques; rango mínimo en cada consumo; R1 (4 SHA); R7 corto |
| 4b tiers y almacén sin volcado | `pipeline/hybrid/{mod,tiers,pages,store}.rs` | propiedad frente a `MemSpoolStore` (B = ∞); crecimiento y recorte; anclaje; asignador |
| 4c volcado | `store.rs` | propiedad con B = 64 KiB y B = 0 con entrada anclada; cuota → `StorageFull`; Z agotado; SHA de R1 de fixtures; nada vivo al final |
| 4d estimación y arnés C6 | `estimate.rs`, `serial.rs`, `tools/convert_parity` | cifras de la Tarea 2 como test; caso wasm32 crudo en `check.sh`; `native_hybrid` = SHA del CLI |
| 5a.1 tiers JS | `spool-tiers.mjs`, falso de Node, tests | `at` > 2³²; escritura corta → 0; pool por clases; vistas no retenidas |
| 5a.2 bindings | `spool_host.rs`, `opfs.rs`, `core_convert.rs`, `run-bindings.mjs`, `check.sh` | paridad de bindings = `native_convert`; cuota falsa → `QuotaExceededError` |
| 5b worker | `converter-worker.js`, `converter-api.mjs`, `package-local.py`, tests | zonas anunciadas antes de crearse; cuota → mensaje y limpieza; `removePartials` hasta 10 s (corrige el reintento de 1 s, que deja parciales en Chrome) |
| 5c Chrome `file://` | `converter-worker.js` | solo RAM, salida a `Blob`, rechazo temprano (D3) |
| 5d preflight | worker, api | `persist()`; cuota frente a volcado + salida; `ready` espera la limpieza (C11) |
| 5e puerta Node | `run-bindings.mjs`, registro | Belvedere d2/d8: SHA = R1, memoria wasm ≤ 1 GiB, cociente ≤ 3 × frente a `native_hybrid` |

Riesgos anotados: recorte a 32 KiB (hasta ~0,16 GiB en Belvedere/8, a medir en la Tarea 6);
estimación empírica (3 trazas); `package-local.py` y un test asumen una sola línea de import;
`check.sh` más lento por compilar `viewer_wasm` en release (interruptor `CHECK_WASM_BINDINGS=0`);
pistas y modelo suponen recorrido en profundidad con N = 1 (revisar en M4/M5).

### Tarea 4: `HybridSpoolStore` en el núcleo

**Archivos:** `crates/viewer_convert_core/src/pipeline/hybrid_store.rs` (y lo que fije la Tarea 3).

- [x] **Paso 1:** tests que fallan: propiedad frente a `MemSpoolStore` con presupuesto de 64 KiB (volcados forzados, también de spools abiertos, y volcado en cada `append` mientras se particiona una entrada fijada; Review Focus 1, C1); escritura corta → error de cuota (Review Focus 2); agotamiento de zonas → error claro (C5); ninguna lectura de un tier mayor que `SPOOL_IO_BUFFER_BYTES` (C9); `convert_serial` con tiers en memoria y presupuesto diminuto da los SHA de R1 de las fixtures; ningún id ni página viva al terminar.
- [x] **Paso 2:** implementar hasta verde; `check.sh`; commit.

### Tarea 5: tiers del host, worker y puerta en Node (C2, C3, C11)

**Archivos:** `crates/viewer_wasm/src/` (tiers y `core_convert.rs`), `examples/web-host/` (módulo de tiers, `converter-worker.js`, `converter-api.mjs`, tests), `tools/convert_parity`.

- [x] **5a, tiers del host (`e8b4b73`, `b90f93f`–`85f1b1e`):** módulo JS de segmentos y zonas con tests Node sobre handles falsos respaldados por `fs` (`at` > 2³², `write` que devuelve 0); paridad Node en `tools/convert_parity` con volcado forzado, en `check.sh`; commit.
- [x] **5b, worker (`1c5e5d6`–`baf5258`):** el worker abre Z zonas antes de convertir y las anuncia como parciales; escritura corta → mensaje de cuota y limpieza (R4, R5); tests Node; commit.
- [x] **5c, Chrome `file://` (`c783b83`):** sin OPFS, solo RAM con aviso; salida en RAM a un `Blob` y validación desde RAM; rechazo temprano por estimación (D3); commit.
- [x] **5d, preflight y limpieza (`e652685`; Firefox sin `persist()` limitado a ~10 GiB; Chrome nunca se rechaza por su estimación):** `persist()` y cuota de Firefox antes de crear zonas; la limpieza de huérfanos termina antes de la primera conversión; commit.
- [x] **5e, puerta en Node (30-10-2026, [registro](../records/benchmarks/M3B_NODE_GATE_2026-10-30.md)): SHA = R1 en d2 y d8, memoria wasm 28–40 MiB, cociente 2,81 × / 2,80 × frente a `native_hybrid` (bajo 3 ×, margen 6–7 %); R7 corto de 4a 0,997.** Belvedere divisiones 2 y 8 a través de los bindings de `viewer_wasm` (shim de `FileReaderSync`, handles sobre `fs`): SHA = R1, `memory.buffer.byteLength` al final y tiempo frente al arnés nativo (C6). Criterios de parada: SHA ≠ R1, cociente > 3 ×, memoria wasm > 1 GiB.

### Tarea 6: Belvedere con un worker en el navegador

**Archivos:** `docs/records/benchmarks/M3B_BELVEDERE_<aaaa-mm-dd>.md` y driver en `docs/records/benchmarks/data/`.

- [x] **Paso 1 (sin otros agentes activos):** Chrome y Firefox por HTTP, Belvedere divisiones 2 y 8: SHA = R1 (`ecdc4889…`, `3aee2890…`); Firefox `file://` divisiones 2; Chrome `file://`: LAS pequeño igual y Belvedere rechazado antes de empezar.
- [x] **Paso 2:** tiempo frente al arnés nativo de C6 en la misma sesión (D2; criterio de parada: > 4 ×) y CLI por defecto como contexto; memoria wasm (C9); pico de PSS (`sample-process-memory.py`), máximo de páginas OPFS y bytes/spools volcados con presupuestos de 2 y 4 GiB (este con `ram_budget` = 8 GB). Registro con la elección entre 2 y 4 GiB (decisión 3) y commit.

### Tarea 7: pendientes de M3a en navegador

- [x] **Paso 1:** cancelar Belvedere durante la jerarquía y durante la escritura: parciales y zonas borrados o limpiados en el siguiente arranque, y reconversión al mismo nombre (Review Focus 4); `formatVersion` y `maxMemoryMiB` en el navegador, con el mismo `Display` de `MemoryBudgetExceeded` que el CLI. Se añade al registro de la Tarea 6 o a uno propio; commit.

### Tarea 8: retirar el conversor antiguo

**Archivos:** borrar `crates/viewer_wasm/src/browser_convert.rs` (mover `OpfsWriter` donde fije la Tarea 3), `crates/viewer_wasm/src/lib.rs`, `examples/web-host/{converter-worker.js, converter-api.mjs, tests/*, package-local.py}`, `examples/web-host/tests/test_package_local.py`, READMEs.

- [x] **Paso 1 (`0cef782`; tras pasar las Tareas 6 y 7 en ambos navegadores, C10):** el motor del núcleo es el único; `engine: "core"` se acepta sin efecto y obsoleto, `"legacy"` da error claro (D4); tests Node; `check.sh` y el paquete `file://` en verde; commit.

### Tarea 9: interfaz del conversor

**Archivos:** `examples/web-host/main.js` (y `index.html` si hace falta).

- [x] **Paso 1 (`04e7306`; la interfaz usa el motor del núcleo, el antiguo sigue en la API hasta la Tarea 8):** exponer las opciones de R2 y `ram_budget` (R9), aviso de Chrome `file://`, progreso y tiempos por fase.
- [x] **Paso 2 (30-10-2026; arreglos en `94600e4`):** verificación en navegador real con Playwright (`ui-verifier`): progreso, cancelación, error de cuota, escritorio y móvil, teclado y foco; commit.

### Tarea 10: documentación y cierre

- [x] **Paso 1 (30-10-2026):** ADR 0017 (zonas paginadas; enmienda las decisiones 2 y 3), spec (decisión 5, S5, M3b), README (fila 7), inventario, READMEs, este plan; `check.sh`; commit.

---

## Después de la etapa 4

Siguiente: M6 (Majadas) con un worker. M4 (pool de preparación y escritor ordenado, S3/S4/S7) y M5
(pool de partición y trabajos por nodo, validación paralela; con D1 los spools pasan por el líder y
hará falta un conductor del plan por pasos) solo si M6 supera 60 min o la experiencia lo pide
(propietario, 30-10-2026). El conversor va después de la escala (R3–R5, V7 final) y la API pública
([orden de trabajo](../requirements.md#orden-de-trabajo-acordado)).
