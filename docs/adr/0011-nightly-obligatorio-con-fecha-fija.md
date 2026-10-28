# 0011 Rust nightly obligatorio con fecha fija

Estado: Aceptada · Fecha: 28-10-2026

## Contexto
El propietario quiere poder adoptar características inestables si hay ganancia medida, con builds reproducibles.

## Decisión
`rust-toolchain.toml` fija `nightly-2026-10-28` (rustfmt, clippy, target `wasm32-unknown-unknown`). La primera ejecución descarga el toolchain (CI necesita rustup ≥ 1.28). La fecha se sube de forma deliberada. No se usa ningún `#![feature]` todavía; se adoptan solo con evidencia de benchmark.

## Alternativas consideradas
| Opción | Descartada porque |
| --- | --- |
| Stable pinneado (1.99.0) | Descartada por decisión del propietario: no permite características inestables. |
| Nightly flotante | Builds no reproducibles. |

## Consecuencias
Mayor riesgo de roturas al subir la fecha; descarga en la primera ejecución.

## Revisar si
Ninguna característica inestable resulta rentable o el mantenimiento del nightly pesa más.

## Evidencia
[rust-toolchain.toml](../../rust-toolchain.toml); [CONTRIBUTING](../../CONTRIBUTING.md#toolchain)
