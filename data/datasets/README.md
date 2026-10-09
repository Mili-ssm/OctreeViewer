# Dataset de escala

La descarga original y el `.oct` generado se conservan localmente, pero se excluyen de Git por su
tamaño. El JSON de metadatos y el procedimiento sí quedan versionados.

## Belvedere Glacier, 2009

- Nube fotogramétrica UAV de una sola ubicación: glaciar Belvedere, Macugnaga, Italia.
- 81.682.329 puntos, RGB, EPSG:7791; el registro informa COPC LAZ de 618,4 MB.
- CC BY 4.0. Registro y atribución: Francesco Ioli et al., [Belvedere Glacier long-term monitoring Open Data](https://doi.org/10.5281/zenodo.19265915).
- Archivo directo: [`belv_2009_digital-aerial_pcd_copc.laz`](https://zenodo.org/records/19265915/files/belv_2009_digital-aerial_pcd_copc.laz?download=1).
- Integridad de la copia descargada: MD5 `197534e677ae2521a97a114bef005ec9` (618.390.994 bytes).
- Metadatos descargados: [`belv_2009_meta.json`](belv_2009_meta.json). La ficha llama al LAZ `belv_2009_digital-aerial_pcd_copc.laz`; el JSON de la encuesta usa el nombre anterior `belv_2009_aerial_pcd.laz`.

El conversor lee COPC secuencialmente y replantea los puntos en su propio octree; no retiene la
jerarquía COPC. La conversión usa los valores por defecto: 262.144 puntos por bloque, error de
cuantización máximo 0,01 y presupuesto controlado estimado de 512 MiB. Los spools se escriben junto
al `.oct`; 55 bytes por punto requieren unos 4,49 GB solo para el spool inicial, y algunas fases
mantienen temporalmente spool de padre e hijos a la vez. Deja al menos 16 GiB libres para esta
ejecución y revisa el espacio temporal antes de repetirla.

## Ejecutar

Desde la raíz del repositorio:

```sh
cargo build --release -p viewer_convert --example real_dataset_bench
/usr/bin/time -v target/release/examples/real_dataset_bench convert \
  data/datasets/belv_2009_digital-aerial_pcd_copc.laz \
  data/datasets/belv_2009_digital-aerial_pcd_copc.oct
/usr/bin/time -v target/release/examples/real_dataset_bench read \
  data/datasets/belv_2009_digital-aerial_pcd_copc.oct \
  data/datasets/belv_2009_digital-aerial_pcd_copc.laz
```

El modo `convert` informa duración total y por fase, puntos/s, bytes del origen y salida, presupuesto
estimado por el conversor y dimensiones omitidas. El modo `read` abre el índice, lee y decodifica cada
bloque por turno, verifica sus CRC y lo descarta antes del siguiente. `read_decode_crc_all_blocks_ms`
incluye lectura, CRC, descompresión y reconstrucción de puntos; no es una medida aislada de LZ4.
`payload_compression_ratio` compara los bytes almacenados de bloques con su representación sin
comprimir; `output_to_input_ratio` compara el `.oct` completo (incluido índice y LOD) con el LAZ que
ya viene comprimido. Resultados medidos y estimaciones de memoria en
[`REAL_DATASET_BENCH.md`](../../docs/REAL_DATASET_BENCH.md).
