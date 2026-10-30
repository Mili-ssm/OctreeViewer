# Puerta Node del hito M3b y R7 corto de las pistas (30-10-2026)

Registro fechado; solo admite añadidos hasta el merge de la rama. Contexto: [plan de la etapa 4](../../plans/conversor-web-paralelo-etapa4.md) (Tarea 4a, Tarea 5e, enmiendas C3 y C6). Referencias SHA: [R1](R1_REFERENCE_SHA_2026-10-29.md). Método R7: [M2 R7](M2_R7_2026-10-29.md). Datos crudos: [JSON](data/m3b-node-gate-2026-10-30.json).

## Entorno

AMD Ryzen AI 9 365 (20 hilos), 30 GiB de RAM, Linux 7.2.13-200.fc44, gobernador `powersave`, perfil ACPI `balanced` (sin `powerprofilesctl`); `rustc 1.101.0-nightly (41089d07a 2026-10-27)`, Node v22.23.1, wasm-bindgen 0.2.129. Máquina sin otros agentes. Antes de cada ejecución se esperó a `loadavg` 1 min < 1,5 y PSI de cpu <= 1,0, y se registraron `loadavg`, PSI cpu/io/memoria, gobernador y perfil (ver JSON). `loadavg` al inicio: 1,1-1,5; PSI de cpu `avg10` máximo 0,17 (R7) y 0,00 (puerta Node); PSI de io máximo 0,52.

Candidato: código de `87ef649`. La rama avanzó con `31c01e6` (solo documentación) y `3a4b1a2` (solo `run-bindings.mjs`); ninguno toca código compilado.

## R7 corto de las pistas del planificador (Tarea 4a)

Base `e03c8fc` (antes de 4a; worktree aparte con su `target/`, ya eliminado) frente al candidato `87ef649`. Perfil `perf`, binarios copiados antes de medir. Comando: `real_dataset_bench convert data/datasets/belv_2009_digital-aerial_pcd_copc.laz OUT.oct 4` (4 workers, divisiones por defecto, 262144 puntos/bloque). Métrica: `conversion_elapsed_ms`. 1 calentamiento por brazo (descartado: 22.834 ms candidato, 23.905 ms base), pares alternados.

| Par | base (ms) | candidato (ms) | cand/base |
| --- | --- | --- | --- |
| 1 | 22.406 | 23.078 | 1,030 |
| 2 | 22.462 | 22.883 | 1,019 |
| 3 | 22.721 | 22.747 | 1,001 |
| 4 | 21.824 | 21.528 | 0,986 |
| 5 | 22.176 | 22.004 | 0,992 |
| 6 | 22.937 | 23.488 | 1,024 |
| 7 | 22.837 | 21.870 | 0,958 |
| 8 | 22.057 | 21.311 | 0,966 |

Los 5 primeros pares dan razón mediana **1,001** (rango 0,986-1,030). El par 1 quedó en 1,0300, en el límite de ±3 %, así que se extendió a 8 pares: razón pareada mediana **0,997** (rango 0,958-1,030); medianas absolutas 22.434 ms (base) y 22.375 ms (candidato). SHA-256 de `.oct`/`.octattrs` en las 19 ejecuciones (ambos brazos, calentamientos incluidos): `ecdc4889778c082e` / `cd3cad4571b24743`, igual a R1. Código de salida 0 en todas.

**Veredicto 4a: pasa** (mediana <= 1,03; SHA = R1). El rango de los pares (±3-4 %) es ruido de la máquina, sin deriva por orden.

## Puerta Node de la Tarea 5e (C3)

### Comandos

```sh
cargo build --release -p viewer_wasm --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir target/convert_parity/pkg target/wasm32-unknown-unknown/release/viewer_wasm.wasm
cargo build --release -p convert_parity --bin native_hybrid
# wasm (bindings reales bajo Node, handles sobre fs; zonas en target/node-gate/w/)
node tools/convert_parity/run-bindings.mjs --input data/datasets/belv_2009_digital-aerial_pcd_copc.laz --divisions D --work-dir target/node-gate/w
# nativo (zonas en target/node-gate/nz/), /usr/bin/time -v para el RSS
target/release/native_hybrid data/datasets/belv_2009_digital-aerial_pcd_copc.laz OUT.oct --divisions D --points-per-block 262144 --spool-budget-bytes 2147483648 --zones-dir target/node-gate/nz [--store mem]
```

Presupuesto de spools 2 GiB, 262144 puntos/bloque, error 0,01, un worker espacial (serie) en ambos lados. Se añadió al runner el modo de entrada única (`--input`, commit `3a4b1a2`, `test(convert): let the bindings runner take one large input`); el modo por defecto no cambia. Orden: nativo, wasm, nativo, wasm por división; una ejecución `--store mem` por división.

### Resultados

| División | Lado | Ronda | ms | SHA = R1 | Memoria wasm (`memory.buffer`) | RSS máx |
| --- | --- | --- | --- | --- | --- | --- |
| 2 | nativo | 1 | 43.840 | sí | n/a | 2,26 GiB |
| 2 | wasm | 1 | 122.561 | sí | 27,9 MiB | 3,03 GiB |
| 2 | nativo | 2 | 43.547 | sí | n/a | 2,26 GiB |
| 2 | wasm | 2 | 122.684 | sí | 27,9 MiB | 3,04 GiB |
| 2 | nativo `--store mem` | 1 | 46.578 | sí | n/a | 6,20 GiB |
| 8 | nativo | 1 | 33.918 | sí | n/a | 2,16 GiB |
| 8 | wasm | 1 | 95.259 | sí | 40,4 MiB | 4,10 GiB |
| 8 | nativo | 2 | 34.244 | sí | n/a | 2,16 GiB |
| 8 | wasm | 2 | 95.257 | sí | 40,4 MiB | 4,10 GiB |
| 8 | nativo `--store mem` | 1 | 34.881 | sí | n/a | 3,77 GiB |

SHA completos (todas las ejecuciones, incluidas las `mem`): divisiones 2 `ecdc4889778c082e…` / `cd3cad4571b24743…`; divisiones 8 `3aee289064d51617…` / `816959b4d1aa3491…`, iguales a R1.

| División | Mediana nativo (ms) | Mediana wasm (ms) | Razón wasm/nativo (2 rondas) | Mediana |
| --- | --- | --- | --- | --- |
| 2 | 43.693 | 122.622 | 2,796; 2,817 | **2,81** |
| 8 | 34.081 | 95.258 | 2,809; 2,782 | **2,80** |

HybridStats (idénticos en nativo y wasm, y en las dos rondas): divisiones 2: 13 zonas estimadas, `spilledBytes` 4.073.405.871 (265 spools), `pagesPeak` 10.152, `overBudgetPeakBytes` 1.179.648, `ramPeakBytes` 2.148.663.296, `zonesReleased` 5, 1.343 bloques. Divisiones 8: 10 zonas, `spilledBytes` 2.620.483.664 (1.025 spools), `pagesPeak` 5.753, `overBudgetPeakBytes` 1.212.416, `ramPeakBytes` 2.148.696.064, `zonesReleased` 2, 5.140 bloques. `livePayloadBytes` y `pagesInUse` terminan en 0.

### Veredictos

| Criterio | Resultado | Veredicto |
| --- | --- | --- |
| SHA = R1 (d2 `ecdc4889…`, d8 `3aee2890…`) | 10 de 10 ejecuciones | **pasa** |
| Razón > 3 × (revisión del dueño) | 2,81 (d2) y 2,80 (d8) | **pasa** (margen del 6-7 %) |
| Memoria wasm > 1 GiB (parada) | 27,9 MiB y 40,4 MiB | **pasa** |

### Límites y observaciones

- **El tiempo wasm incluye la validación** (`convert()` lee las salidas de vuelta, ~1,3 s en el CLI) y el nativo no; por tanto la razón de trabajo equivalente es algo menor que la medida. No se separó.
- **La memoria wasm no es el coste real**: los spools en RAM viven en `ArrayBuffer`s de JS, fuera del `memory` de wasm. El RSS de Node llegó a 3,0 GiB (d2) y 4,1 GiB (d8), frente a 2,3 y 2,2 GiB del nativo; conviene vigilarlo en la Tarea 6 (límites de pestaña del navegador). El máximo (`VmHWM`) coincide con el RSS final.
- El RSS de las ejecuciones `--store mem` (6,2 y 3,8 GiB) es solo contexto: no usan zonas.
- Dos rondas por celda; las razones (2,78-2,82) son estables, pero no es una distribución. Una máquina, una sesión, `release` (opt-level "s", LTO fat), sin Chrome/Firefox; las zonas son ficheros en `target/` (disco NVMe, caché de página del SO), no OPFS.
- Una ejecución preliminar de wasm d2 (127.967 ms, sin registro de carga) sirvió para comprobar el runner y se descartó.
