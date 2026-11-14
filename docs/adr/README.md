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
| [0007](0007-limites-por-defecto-del-lector.md) | El lector aplica límites por defecto configurables | Aceptada |
| [0008](0008-perfiles-release-y-perf.md) | `release` optimiza tamaño; `perf` optimiza velocidad para benchmarks | Aceptada |
| [0009](0009-transporte-web-por-blob-local.md) | El transporte web es Blob local, sin HTTP Range | Sustituida por [0023](0023-fuentes-locales-y-remotas.md) |
| [0010](0010-migracion-a-wgpu-30-y-glam-0-34.md) | Migrar el renderer a wgpu/naga 30 y glam 0.34 | Aceptada |
| [0011](0011-nightly-obligatorio-con-fecha-fija.md) | Rust nightly obligatorio con fecha fija | Aceptada |
| [0012](0012-validacion-previa-de-cabeceras-las-laz.md) | Validar cabeceras LAS/LAZ antes de que las crates las parseen | Aceptada |
| [0013](0013-modulos-experimentales-tras-feature-experimental.md) | Los módulos experimentales van tras la feature `experimental` | Aceptada |
| [0014](0014-nucleo-de-conversion-sin-io.md) | Núcleo de conversión sin I/O compartido por CLI y navegador | Aceptada |
| [0015](0015-pool-de-workers-sin-memoria-compartida.md) | Paralelismo de conversión web por pool de workers sin memoria compartida | Aceptada (enmendada por [0024](0024-hilos-wasm-opcionales-con-coop-coep.md)) |
| [0016](0016-norma-portatil-en-el-error-de-cuantizacion.md) | Norma portátil en el error de cuantización (sin libm en la salida) | Aceptada |
| [0017](0017-zonas-opfs-paginadas-para-spools-web.md) | Los spools del conversor web van en segmentos de JS y zonas OPFS paginadas | Aceptada |
| [0018](0018-worker-de-construccion-por-defecto.md) | El worker de construcción se activa por defecto | Aceptada |
| [0019](0019-cubos-por-nodo-residentes-en-gpu.md) | Cubos 3D por nodo, independientes de la vista y residentes en GPU (R3) | Aceptada |
| [0020](0020-piramide-lod-densa.md) | La pirámide LOD es densa: un nivel por cada ×2 de densidad (R4) | Aceptada |
| [0021](0021-rejilla-global-determinista-y-crs.md) | Rejilla de cuantización global determinista anclada en el CRS | Aceptada |
| [0022](0022-alcance-y-congelacion-de-v7-final.md) | V7 final incluye R4, R5 y la rejilla global, y se congela una vez | Aceptada |
| [0023](0023-fuentes-locales-y-remotas.md) | El visor lee fuentes locales (Blob) y remotas (HTTP Range) | Aceptada |
| [0024](0024-hilos-wasm-opcionales-con-coop-coep.md) | Hilos wasm opcionales cuando la página está aislada (COOP/COEP) | Aceptada |
| [0025](0025-codificador-lz4-propio.md) | Codificador LZ4 propio en `viewer_format` | Aceptada |
| [0026](0026-api-publica-licencia-y-version-1.md) | API pública primero, licencia MIT OR Apache-2.0 y publicación solo en la 1.0 | Aceptada |
| [0027](0027-celdas-por-nodo-en-pool-paginado.md) | R3: celdas por nodo en un pool de GPU paginado con máscaras de cobertura | Aceptada (14-11-2026) |
| [0028](0028-r4-antes-que-r3.md) | R4 (pirámide densa) antes que R3 | Aceptada |
| [0029](0029-octree-global-por-bits-de-enteros.md) | Octree global sobre coordenadas enteras con cuanto binario (precisa 0021) | Aceptada |
| [0030](0030-jerarquia-paginada-por-anclas.md) | Jerarquía paginada por niveles desde anclas de prefijo (R5) | Aceptada |
| [0031](0031-celdas-por-nodo-en-v7-5.md) | Celdas por nodo en v7.5: pirámide por nodo, coordenadas por página del pool, claves de 64 bits (sustituye en parte a 0027) | Aceptada (enmendada por 0032) |
| [0032](0032-hojas-adaptativas-e-instancia-de-12-b.md) | Hojas hasta separar posiciones, clave regional más allá de 21 bits e instancia de 12 B (enmienda a 0031) | Aceptada (instancia enmendada por 0035) |
| [0033](0033-gpu-minima-4-gb-y-corte-ajustado-al-pool.md) | GPU mínima de 4 GB y corte ajustado al pool (sustituye P2) | Aceptada (enmendada por 0040) |
| [0034](0034-nivel-por-octante-con-arista-real.md) | Nivel por octante con la arista proyectada real y tolerancia de RQ-09 dentro del dataset (enmienda a 0031) | Aceptada |
| [0035](0035-instancia-de-16-b-con-w-16.md) | Instancia de 16 B con W = 16 y compactación con Hi-Z antes de la Tarea 6 (enmienda a 0032) | Aceptada (oclusión enmendada por 0036) |
| [0036](0036-hi-z-por-celda.md) | Hi-Z por celda en dos fases sobre el pool (enmienda a 0035) | Aceptada |
| [0037](0037-corte-ajustado-al-presupuesto-de-cpu.md) | Corte de celdas ajustado al presupuesto de CPU (extiende 0033) | Aceptada (enmendada por 0038 y 0039) |
| [0038](0038-residencia-por-celdas.md) | Residencia por celdas en la ruta de celdas (enmienda a 0037) | Aceptada |
| [0039](0039-hijos-en-curso-y-bloqueo-retenido.md) | Hijos en curso y bloqueo retenido en el ajuste por presupuesto de CPU (enmienda a 0037) | Aceptada |
| [0040](0040-pool-de-512-mib-y-corte-ajustado-al-pool.md) | Pool de celdas de 512 MiB y corte de nodos ajustado al pool (enmienda a 0033) | Aceptada |
| [0041](0041-orden-de-filas-libre-por-bloque.md) | Orden de filas libre por bloque en V7 y escritor híbrido (precisa 0022) | Aceptada |
| [0042](0042-escritor-hibrido-en-la-identidad-final-de-v7.md) | El escritor híbrido entra en la identidad final de V7, sin revisión `-pre` propia (enmienda a 0005) | Aceptada |
