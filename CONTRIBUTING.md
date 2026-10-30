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
`git diff --check HEAD`. Por defecto usa `--offline`; `CHECK_LOCKED=1` usa `--locked` (así lo
ejecuta `.github/workflows/ci.yml`).

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
cd crates/viewer_wasm
wasm-pack build --target web --out-dir ../../pkg
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
- **V5 predeterminado**: sigue siendo la salida por defecto durante la transición a V7, que
  concentra el desarrollo; no se crean revisiones V5/V6 nuevas.
- **Literales de versión**: `FormatVersion::V<n>` solo vive en `viewer_format`; el resto usa su
  API de capacidades (`DEFAULT_OUTPUT`, `WRITABLE`, `supports_block_checkpoint()`), así añadir una
  revisión no toca otros crates. Lo vigila `scripts/check_version_literals.sh`.
- **Registros inmutables**: lo de `docs/records/` no se edita (salvo corregir enlaces rotos);
  nueva evidencia va en un registro nuevo o en `format-v7-evidence.md`, que se amplía.
- **Fechas** `dd-mm-aaaa`; documentación en español salvo el README del host web (inglés).
- **Decisiones**: las de diseño duraderas se registran en [docs/adr/](docs/adr/README.md); una aceptada se sustituye con un ADR nuevo, no se edita.
- **Un hecho, un documento**: el resto enlaza; dónde va cada tipo de documento, en el
  [índice](docs/README.md#dónde-va-cada-cosa). Las prioridades activas solo en «Prioridades
  pendientes» del README; las preguntas para el propietario, en
  [requisitos](docs/requirements.md#preguntas-abiertas).
- **Benchmarks**: las cifras no se trasladan entre revisiones, perfiles, navegadores, datasets ni
  equipos; registrar el perfil. `fast` es para tests; `perf` (`opt-level` 3, LTO thin) para
  benchmarks nativos; `release` (`opt-level` "s", LTO completo) es el de producción. No son
  intercambiables.
