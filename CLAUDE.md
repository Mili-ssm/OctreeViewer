# Octree Viewer — reglas para agentes

Visor LiDAR Rust/WASM/WebGPU. Estado y prioridades: `README.md`; índice: `docs/README.md`;
guía de desarrollo: `CONTRIBUTING.md`.

- Antes de dar algo por terminado, ejecutar `scripts/check.sh` (14 pasos; `CHECK_LOCKED=1` para
  `--locked`). No sustituirlo por comandos sueltos.
- El toolchain nightly está fijado en `rust-toolchain.toml`; no cambiar la fecha sin decisión
  explícita. No añadir `#![feature]` sin evidencia de benchmark.
- Los registros en `docs/records/` son inmutables; no editarlos.
- V5 sigue siendo el formato de salida predeterminado durante la transición a V7. No desarrollar
  nuevas revisiones V5/V6.
- Los literales `FormatVersion::V<n>` solo en `crates/viewer_format`; fuera, usar su API de
  capacidades.
- Fechas en `dd-mm-aaaa`. Documentación en español (el README del host web, en inglés).
- Las cifras de benchmark no se trasladan entre revisiones, perfiles (`perf` vs `release`),
  navegadores ni equipos.
- Un hecho, un documento: enlazar en lugar de copiar. Las prioridades activas viven solo en la
  tabla «Prioridades pendientes» del README.
