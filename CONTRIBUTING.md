# Contribuir

## Toolchain

Rust **nightly obligatorio** (decisión del propietario, 28-10-2026), fijado en
`rust-toolchain.toml` como `nightly-2026-10-28` con `rustfmt`, `clippy` y el target
`wasm32-unknown-unknown`. rustup lo descarga en la primera ejecución (CI necesita rustup ≥ 1.28).
Subir la fecha es una decisión deliberada, no automática. Aún no se usa `#![feature]`: las
características inestables se adoptan solo con evidencia de benchmark.

## Checks

```sh
scripts/check.sh
```

Es el único punto de entrada (23 pasos con el dataset de Belvedere y fuzz; menos sin ellos): tests y clippy con features por defecto y con
`--all-features`, clippy `wasm32`, el núcleo `viewer_convert_core` para wasm32 (`cargo check`, clippy y `cargo tree` sin `laz-parallel`/`rayon`/`ctrlc`; alias `cargo check-core-web`), paridad wasm32/nativo bajo Node de bloques (`format parity wasm32`, y `format parity real blocks` si existe Belvedere) y de la conversión completa (`convert parity wasm32`, `tools/convert_parity`), `cargo doc` con `-D warnings`, `fmt`, tests Node, tres suites
Python, comprobación de enlaces (`scripts/check_docs_links.py`, excepciones en
`scripts/docs-links-allowlist.txt`), índices y líneas `Estado:` de ADR y planes (`scripts/check_docs_index.py`), literales de versión (`scripts/check_version_literals.sh`) y
`git diff --check HEAD`. Por defecto usa `--offline`; `CHECK_LOCKED=1` usa `--locked`.

**Sin CI** (decisión del propietario, 15-11-2026): es un proyecto personal y los checks se ejecutan
solo en local. No se añaden workflows de GitHub Actions; Actions solo ejecuta el despliegue gestionado de
GitHub Pages de la demo (acciones de GitHub, gratis en un repositorio público; [ADR 0045](docs/adr/0045-demo-experimental-publica-antes-de-la-1-0.md)).

`convert parity bindings` ejecuta los bindings reales de `viewer_wasm` (`wasm-bindgen`) en Node contra el CLI; se omite solo si la versión del CLI `wasm-bindgen` difiere de `Cargo.lock` o con `CHECK_WASM_BINDINGS=0`.

No cubre: pruebas GPU ignoradas (requieren adaptador o Vulkan por software), probes de navegador
(`*.browser.mjs`, requieren bindings recientes y orquestación) ni presentación visual. Para cambios en el host web, comprobar además con
Playwright los flujos afectados, consola y escritorio/móvil. Readback nativo no es evidencia de
presentación en todos los navegadores. Borrar de `target/` conversiones y temporales que ya no
hagan falta; conservar informes y muestras que documenten resultados.

### Fuzzing (opcional)

Objetivos `cargo-fuzz` para el lector de formato y el preflight LAS/LAZ; no se ejecutan en
`scripts/check.sh` ni en CI, pero `check.sh` sí comprueba que el crate compila y está formateado
(`cargo check`/`cargo fmt --check` sobre `fuzz/`). `FUZZ_CHECK=0 scripts/check.sh` omite esos dos
pasos cuando no hay nightly o toolchain C++ para compilar `libfuzzer-sys`. Uso y gestión de
crashes: [fuzz/README.md](fuzz/README.md).

## Features Cargo

Apagadas por defecto; lista y alcance en el [inventario](docs/reference/inventory.md#componentes-y-puntos-de-entrada).

## Compilar WASM

Detalle, API y probes: [host web](examples/web-host/README.md).

```sh
scripts/build-wasm-bindings.sh   # cargo + wasm-bindgen (versión de Cargo.lock) en pkg/
```

Paquete local para `file://`:

```sh
cargo build --release --offline -p viewer_wasm --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir pkg target/wasm32-unknown-unknown/release/viewer_wasm.wasm
python3 examples/web-host/package-local.py
```

`pkg/` y el HTML/ZIP exportado son artefactos: regenerar tras cambios.

## Convenciones

Fuente única de las reglas.

- Commits: [Conventional Commits](https://www.conventionalcommits.org/) (`docs:`, `build:`, `fix:`…).
- **Solo V8**: V8 es el único formato que se lee y se escribe ([referencia](docs/reference/format/FORMAT_V8.md),
  [plan de congelado](docs/records/archive/formato-v8-congelado.md)); los formatos anteriores dan el error «retirado, reconvierte».
  Un cambio de bytes de V8 necesita un ADR y el visto bueno del propietario.
- **Literales de versión**: `FormatVersion::V<n>` solo vive en `viewer_format`; el resto usa su
  API de capacidades (`DEFAULT_OUTPUT`, `WRITABLE`, `supports_block_checkpoint()`), así añadir una
  revisión no toca otros crates. Lo vigila `scripts/check_version_literals.sh`.
- **Registros inmutables**: lo de `docs/records/` no se edita (salvo corregir enlaces rotos y su
  [índice](docs/records/README.md), donde cada registro nuevo añade su fila);
  nueva evidencia va en un registro nuevo y fechado (`format-v7-evidence.md` es histórico y ya no se amplía).
- **Fechas** `dd-mm-aaaa`; documentación en español salvo `README.md`, `SECURITY.md` y `examples/web-host/README.md` y `PROBES.md` (inglés).
- **Decisiones**: las de diseño duraderas se registran en [docs/adr/](docs/adr/README.md); una aceptada se sustituye con un ADR nuevo, no se edita.
- **Un hecho, un documento**: el resto enlaza; dónde va cada tipo de documento, en el
  [índice](docs/README.md#dónde-va-cada-cosa). El resumen de prioridades activas, en «Prioridades
  pendientes» de [docs/estado.md](docs/estado.md#prioridades-pendientes); las preguntas para el propietario, en
  [requisitos](docs/requirements.md#preguntas-abiertas) con su issue; lo pendiente, en
  [issues](#issues-y-prs).
- **Benchmarks**: las cifras no se trasladan entre revisiones, perfiles, navegadores, datasets ni
  equipos; registrar el perfil. `fast` es para tests; `perf` (`opt-level` 3, LTO thin) para
  benchmarks nativos; `release` (`opt-level` 3, LTO completo; desde el 02-11-2026, antes "s") es el de producción. No son
  intercambiables.
- **Clippy**: los lints viven en `[workspace.lints]` del `Cargo.toml` raíz y cada crate los hereda
  con `[lints] workspace = true`. Política: `pedantic` en `warn` con excepciones (`allow`)
  en el propio `Cargo.toml`, cada una con su motivo en un comentario (los `cast_*` comparten uno); lints de
  `restriction` elegidos uno a uno, nunca el grupo; `disallowed_methods`/`disallowed_types` en `deny` (su configuración sigue en el
  `clippy.toml` de cada crate). Una excepción local usa `#[expect(clippy::lint, reason = "…")]`;
  `#[allow(…, reason = "…")]` solo donde `expect` no puede funcionar (un lint que salta solo en
  algunos targets o binarios de test), y todo `unsafe` lleva un comentario `// SAFETY:`. `viewer_render_wgpu` y
  `viewer_wasm` se suman al terminar la fase 2b; hasta entonces no heredan los lints.

## Issues y PRs

Desde el 15-11-2026 el trabajo pendiente vive en GitHub
([`Mili-ssm/OctreeViewer`](https://github.com/Mili-ssm/OctreeViewer)); los `.md` guardan lo duradero
(requisitos, ADR, planes, registros) y enlazan la issue. Las reglas suponen un único propietario que trabaja con agentes LLM: las decisiones que le corresponden se etiquetan `pregunta-propietario`.

| Qué | Dónde | Regla |
| --- | --- | --- |
| Fallo, mejora o tarea pendiente | Issue (plantilla `fallo` o `mejora`) | Una por hallazgo, antes de cambiar código salvo arreglos triviales del mismo cambio; criterios de aceptación comprobables |
| Decisión que espera al propietario | Issue `pregunta-propietario` | Enlaza su `OQ-` de [requisitos](docs/requirements.md#preguntas-abiertas) o el plan o ADR de origen; la respuesta se registra allí o en un ADR y se cierra la issue |
| Iniciativa de varias tareas | Issue `proyecto` («Proyecto · …») con sub-issues | El diseño va en `docs/plans/`; la issue lleva el progreso |
| Evolutivo | PR a `main` desde `feat/…` | Cuerpo con `Closes #N`; una PR por intención |
| Experimento | Propuesta: issue con la plantilla `experimento` (opcional). Ejecución: PR **en borrador** desde `explore/…` o `experiment/…`, etiqueta `experimento` | La PR lleva hipótesis, método, resultado y decisión, enlaza su registro y cierra la issue de la propuesta si la hay. Descartado → cerrar sin fusionar (la rama queda); se integra solo con el visto bueno del propietario |
| Hallazgo de revisión de severidad baja que no se arregla | Issue `aparcado` | Una por tarea o plan, con la lista de puntos |

**Etiquetas** (una de cada grupo): área `formato` · `conversor` · `visor` · `web` · `api` · `docs` ·
`herramientas`; tipo `bug` · `enhancement` · `experimento`; prioridad `prioridad:alta` · `prioridad:media` ·
`prioridad:baja`. Especiales: `pregunta-propietario`, `proyecto`, `aparcado`.

**Agentes:** la regla de quién usa `gh` está en [AGENTS.md](AGENTS.md); si `gh` falla, anotar «issue
pendiente» en el commit.