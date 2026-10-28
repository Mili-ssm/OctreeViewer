# Evaluación y plan de mejora — 28-10-2026

Registro inmutable de la evaluación realizada sobre la rama `chore/assessment-2026-10-28`. Sustituye como referencia vigente a la [evaluación del 24-10-2026](CODE_ASSESSMENT_2026-10-24.md); las prioridades activas siguen en el [README](../../../README.md#prioridades-pendientes).

## Método

1. Revisiones especializadas en paralelo: arquitectura, coherencia documentación↔código, estructura de la documentación, QA y seguridad.
2. Plan de 15 tareas ejecutado en secuencia (un implementador a la vez), cada tarea con revisión independiente; cuando el cambio era mecánico (mover tests, mover documentos) la revisión fue una comprobación mecánica de equivalencia en lugar de una lectura.
3. Las desviaciones del plan se resolvieron con decisiones del coordinador (sección Decisiones).

La tarea 15 (evaluación de características nightly) estaba en curso y no forma parte de este registro.

## Hallazgos por área

| Área | Hallazgo | Resolución |
| --- | --- | --- |
| Arquitectura | Política de versiones dispersa en convert/wasm; contrato LAS duplicado; `viewer_convert/lib.rs`, `viewer_format` reader/writer y `viewer_wasm/bindings.rs` monolíticos; módulos experimentales siempre compilados. | `FormatVersion::{DEFAULT_OUTPUT, WRITABLE, supports_block_checkpoint}` ([ADR 0006](../../adr/0006-politica-de-version-solo-en-viewer-format.md)); `viewer_core::las`; división en módulos con ciclos eliminados (queda la arista `index_codec`→reader, no un ciclo); `bindings/` como directorio; feature `experimental` ([ADR 0013](../../adr/0013-modulos-experimentales-tras-feature-experimental.md)). |
| Documentación | README con 205 líneas y tabla de documentos duplicada; V7.3 descrita sin checkpoint EPT; ancla rota; inventario fechado 14-10 y con tabla duplicada; sin guía de contribución ni CLAUDE.md; sin índice. | Nueva estructura `reference/explanation/records`, `docs/README.md`, `CONTRIBUTING.md`, `CLAUDE.md`, READMEs por crate, inventario refrescado, README a 123 líneas. |
| Calidad | Sin script único de checks ni CI; `--all-features` nunca se ejecutaba (5 tests `async-reader`); tests inline de ~8k líneas en código de producción; sin barridos de corrupción. | `scripts/check.sh`, `.github/workflows/ci.yml`, comprobador de enlaces y de literales de versión; tests movidos a ficheros hermanos; barridos de bits y truncado por revisión y de journal de checkpoints. |
| Seguridad | Reservas de memoria dependientes de cabeceras LAS/LAZ hostiles; empaquetado y exportación CSV del host web; recuperación del worker del conversor. | `preflight_las` ([ADR 0012](../../adr/0012-validacion-previa-de-cabeceras-las-laz.md)); escape en `package-local` y fórmulas CSV; recreación del worker. |
| Toolchain | Sin versión fijada. | Nightly fechado ([ADR 0011](../../adr/0011-nightly-obligatorio-con-fecha-fija.md)) y dependencias compatibles actualizadas. |

## Cambios (commits desde `f43416d`)

Línea base `df67d1d`, luego:

- Refactor: `4901f03` política de versiones; `5fc4d6f` contrato LAS; `938022b` fs/CRC compartidos; `e10eab6`, `d926cfe` división de `viewer_convert`; `8ff0baf`, `4b7c699` división de `viewer_format`; `b3aac4e` bindings WASM; `2bfc648` feature `experimental`.
- Tests: `dc97f1f` tests a ficheros hermanos; `227a2a7`, `24c62af`, `dec3909` barridos de corrupción y journal.
- Seguridad: `3f136f5`, `91254d8`, `93de5c2` preflight LAS/LAZ; `e25e1fa` host web.
- Build y CI: `61250ed`, `0c6e79f` script, comprobadores y workflow; `1686f5a` nightly fijado; `1ed746a`, `a700998` dependencias.
- Documentación: `05a0b61` reorganización; `ef223f4`, `f9ab732` índice, guías e inventario; ADR y este registro en el commit siguiente.

Lista completa: `git log --oneline f43416d^..HEAD`.

## Decisiones del coordinador

- `visual_scale_for_bounds` conserva `Some(1.0)` con extensión cero: preservar comportamiento prevalece sobre la suposición del plan.
- Tests de movimiento (tarea 4) y documentos movidos (tarea 12): verificación mecánica de equivalencia.
- Barridos de corrupción: un bit con CRC reparado puede decodificar otros nombres/XYZ por diseño (el CRC protege contra corrupción accidental, no autentica); se eliminó el test ignorado que afirmaba lo contrario. El barrido sin reparar mantiene el oráculo completo (sin pánico y error o datos idénticos). El fixture se redujo a 3 bloques × 24 puntos × 5 columnas para cumplir el presupuesto de tiempo.
- Preflight: la cota del número de chunks es `min(cota de cabecera, (offset_tabla − (offset_datos + 8)) / max(longitud de registro, mínimo del formato))`, para modo fijo y variable, porque cada chunk empieza con al menos un punto crudo; la meta (acotar reservas) prevalece sobre la fórmula del plan. Una primera versión rechazaba Albuera (LAS 1.2, 508 809 556 puntos) y Majadas; se corrigió el desplazamiento de `chunk_size` (+14→+12).
- Sin `rust-toolchain.toml` con stable: sustituido por la decisión del propietario (28-10-2026) de nightly fechado obligatorio.
- Implementadores en secuencia, no en paralelo.

## Verificación

- `scripts/check.sh`: 14 pasos en verde (tests y clippy con y sin `--all-features`, clippy `wasm32`, `cargo doc`, `fmt`, Node, tres suites Python, enlaces, literales de versión, `git diff --check`).
- Atributos: `wide_schema_requires_explicit_reader_limits` (`viewer_format/src/writer_tests.rs`) cubre 257 atributos en cada revisión de `WRITABLE`: el writer acepta el esquema, `FormatReaderLimits` por defecto lo rechaza y límites propios lo abren ([ADR 0007](../../adr/0007-limites-por-defecto-del-lector.md)).
- Preflight: 228 archivos LAS/LAZ reales (muestras, Belvedere COPC 1.4, teselas EPT, Albuera, Majadas) y 2100 generados con `las::Writer` pasan; 3 PoC manipulados se rechazan. Alcance: la cabecera (`viewer_core::las`) acota EVLR y el número de chunks; la ruta nativa además acota las entradas decodificadas de la tabla de chunks en `viewer_convert` (`laz::laszip::ChunkTable`, necesario porque el descompresor paralelo reserva según `point_count`/`byte_count` con signo extendido); la ruta WASM usa el descompresor secuencial y no depende de esas entradas. Los 228 archivos reales también pasan la comprobación de entradas.
- Recuento de tests por tarea: 404 por defecto y 425 con `--all-features` tras la tarea 8; 408/0/6 tras la tarea 9.

## Pendiente

- Reportar a upstream los problemas de reservas de `las`/`laz` (EVLR, tabla de chunks) y el desbordamiento en compilación debug con conteos u64 enormes (`raw/header.rs:359`; release envuelve). La diferencia de fuentes `las` 0.11.1→0.11.2 es formato y un `use std::f64` eliminado, así que ese desbordamiento no cambia en 0.11.2, y las reservas EVLR (`las`) y de entradas/capacidad de la tabla de chunks (`laz`) siguen sin corregir upstream. Acción abierta: reportarlos.
- Preflight: la cota aún permite ~0,8× el tamaño del fichero en la tabla de chunks (proporcional, no amplificación); sin tests de EVLR con `start == 0` ni de precedencia del conteo legacy.
- Duplicación de `WRITABLE.filter(supports_block_checkpoint)` en `ept_checkpoint.rs` y `main.rs`; `expect()` en `ept_checkpoint.rs` (código de biblioteca); texto de ayuda con «v7.x-pre» fijo.
- `convert/lib.rs` recalcula la extensión para el error `InvalidVisualScale`; docs de `visual_scale_for_bounds` y `las_attribute_schema` escuetas.
- Test de CRC en `ept_checkpoint` en lugar de `crc32.rs`; helpers de directorios temporales y fixtures de corrupción aún con duplicados (`spatial.rs`, `corruption.rs`, `reader_tests.rs`); el test de campos extremos acepta cualquier `Err`.
- Aristas restantes: `index_codec`→reader (`read_attribute_values`), re-exports `pub(crate)` en `writer.rs`, `create_offscreen_with_config` innecesariamente `pub(super)`, importaciones vía `use super::{…}` en `ept.rs`/`spatial.rs`.
- `collect-benchmarks.py:56` usa `match` con `$` (acepta salto de línea final): usar `fullmatch`; falta test de error sin petición activa.
- Comprobador de enlaces: la minúscula de slugs con subrayado tras carácter no ASCII pierde el `_`.
- Crate READMEs: líneas largas y enlace duplicado en `viewer_core`.
- Tarea 15 (características nightly): en curso, fuera de este registro.
