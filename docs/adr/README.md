# Registros de decisiones (ADR)

MADR corto: una decisión por registro. Una decisión aceptada no se edita: se sustituye con un ADR nuevo. Orden por número; los números no se reutilizan.

| Nº | Decisión | Estado |
| --- | --- | --- |
| [0001](0001-pareja-separada-oct-octattrs.md) | La salida es una pareja separada `.oct` / `.octattrs` | Aceptada |
| [0002](0002-v5-predeterminado-en-transicion.md) | V5 sigue siendo la salida predeterminada durante la transición a V7 | Aceptada |
| [0003](0003-v6-final-y-pre5-rechazados.md) | V6 final y pre5 permanecen rechazados | Aceptada |
| [0004](0004-retirar-v5-v6-tras-v7.md) | Retirar V5/V6 al completar V7 | Aceptada |
| [0005](0005-identidades-exactas-v7n-pre-por-candidato.md) | Cada candidato de writer V7 tiene identidad exacta `v7.N-pre` | Aceptada (revisar) |
| [0006](0006-politica-de-version-solo-en-viewer-format.md) | La política de versiones vive solo en `viewer_format` | Aceptada |
| [0007](0007-limites-por-defecto-del-lector.md) | El lector aplica límites por defecto configurables | Propuesta |
| [0008](0008-perfiles-release-y-perf.md) | `release` optimiza tamaño; `perf` optimiza velocidad para benchmarks | Aceptada |
| [0009](0009-transporte-web-por-blob-local.md) | El transporte web es Blob local, sin HTTP Range | Aceptada |
| [0010](0010-migracion-a-wgpu-30-y-glam-0-34.md) | Migrar el renderer a wgpu/naga 30 y glam 0.34 | Aceptada |
| [0011](0011-nightly-obligatorio-con-fecha-fija.md) | Rust nightly obligatorio con fecha fija | Aceptada |
| [0012](0012-validacion-previa-de-cabeceras-las-laz.md) | Validar cabeceras LAS/LAZ antes de que las crates las parseen | Aceptada |
| [0013](0013-modulos-experimentales-tras-feature-experimental.md) | Los módulos experimentales van tras la feature `experimental` | Aceptada |
| [0014](0014-nucleo-de-conversion-sin-io.md) | Núcleo de conversión sin I/O compartido por CLI y navegador | Aceptada |
| [0015](0015-pool-de-workers-sin-memoria-compartida.md) | Paralelismo de conversión web por pool de workers sin memoria compartida | Aceptada |
