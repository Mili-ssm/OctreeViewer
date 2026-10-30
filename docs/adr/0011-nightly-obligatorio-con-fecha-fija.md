# 0011 Rust nightly obligatorio con fecha fija

Estado: Aceptada · Fecha: 28-10-2026

## Contexto
El propietario quiere poder adoptar características inestables si hay ganancia medida, con builds reproducibles.

## Decisión
`rust-toolchain.toml` fija `nightly-2026-10-28` (rustfmt, clippy, target `wasm32-unknown-unknown`). La primera ejecución descarga el toolchain (CI necesita rustup ≥ 1.28). La fecha se sube de forma deliberada. No se usa ningún `#![feature]` todavía; se adoptan solo con evidencia de benchmark.

## Alternativas consideradas
| Opción | Descartada porque |
| --- | --- |
| Stable pinneado (1.99.0) | Superada por la decisión del propietario (28-10-2026; el pin stable fue una decisión previa del coordinador el mismo día): no permite características inestables. |
| Nightly flotante | Builds no reproducibles. |

## Consecuencias
Mayor riesgo de roturas al subir la fecha; descarga en la primera ejecución.

## Revisar si
Ninguna característica inestable resulta rentable o el mantenimiento del nightly pesa más.

## Evidencia
[rust-toolchain.toml](../../rust-toolchain.toml); [CONTRIBUTING](../../CONTRIBUTING.md#toolchain)

## Revisión 30-10-2026
El propietario mantiene el nightly con fecha fija (CON-07 de [requisitos](../requirements.md)). Los hilos wasm opcionales ([ADR 0024](0024-hilos-wasm-opcionales-con-coop-coep.md)) necesitan `build-std` con atomics, que exige nightly.
