# viewer_convert_core

Núcleo puro de la conversión LAS/LAZ a `.oct`/`.octattrs`, compartido por el CLI nativo y el
navegador: opciones, errores, planificador espacial, partición raíz por 1–8 rangos de chunks LAZ o
registros LAS, y preparación de bloques.
Invariantes: sin `std::fs`, hilos, reloj, `process`, `rayon`, `laz-parallel` ni funciones float
respaldadas por libm (`hypot`, `powf`, `exp`…; [ADR 0016](../../docs/adr/0016-norma-portatil-en-el-error-de-cuantizacion.md)); compila para
`wasm32-unknown-unknown`. Los hosts aportan E/S por `SpoolStore`, `SourceFactory` y `Clock`.
Entrada: `convert_serial` y `validate_output_serial` (ruta serie sobre `Write + Seek`),
`ConvertOptions::with_root_partition_ranges` y `LazChunkPointSource` (`src/pipeline/laz_chunks.rs`).
Verificación: `clippy.toml` (lint denegado) y los pasos de núcleo y paridad de
[`scripts/check.sh`](../../scripts/check.sh) (`cargo check-core-web` para el check wasm32);
contexto en el [plan de la etapa 3](../../docs/explanation/conversor-web-paralelo-etapa3.md).
