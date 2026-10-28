# Octree Viewer — reglas para agentes

Visor LiDAR Rust/WASM/WebGPU. Estado y prioridades: `README.md`; índice: `docs/README.md`.
Motivos y detalle de cada regla: [CONTRIBUTING.md](CONTRIBUTING.md) («Toolchain», «Convenciones»).

- Antes de dar algo por terminado, ejecutar `scripts/check.sh` (todos los pasos).
- No cambiar la fecha del nightly de `rust-toolchain.toml` ni añadir `#![feature]` sin decisión
  o evidencia.
- No editar `docs/records/`.
- V5 es la salida predeterminada; no crear revisiones V5/V6 nuevas.
- `FormatVersion::V<n>` solo en `crates/viewer_format`.
- Fechas `dd-mm-aaaa`; docs en español.
- No trasladar cifras de benchmark entre revisiones, perfiles (`perf`/`release`) ni equipos.
- Un hecho, un documento: enlazar, no copiar.
