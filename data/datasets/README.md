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
al `.oct`; 55 bytes por punto requieren unos 4,49 GB solo para el spool inicial. En una conversión
medida, los temporales alcanzaron 8,37 GiB observados mientras algunas fases mantenían spools de
padres e hijos a la vez. La medida usó muestreo de 100 ms, por lo que puede omitir picos breves; deja
al menos 16 GiB libres y revisa el espacio disponible antes de repetirla. Resultados y método en
[`REAL_DATASET_BENCH.md`](../../docs/REAL_DATASET_BENCH.md).

## SensOFOREST: Albuera UAV y Majadas TLS

Dos vuelos del mismo [registro Zenodo 20000277](https://zenodo.org/records/20000277) para pruebas de
mayor volumen y perfiles de escaneo diferentes. Los LAZ originales ocupan **5.648.751.259 B (5,65 GB
decimales)** combinados, por debajo del límite de 50 GB solicitado. La copia local está en
`data/datasets/sensoforest/`; el patrón `*.laz` mantiene estos archivos fuera de Git.

| Escaneo | Archivo | Tamaño | Puntos en cabecera | CRS del README del dataset |
| --- | --- | ---: | ---: | --- |
| Albuera UAV LiDAR | `SensOFOREST_2026_02_22_Albuera_Pointcloud_UAV_LiDAR_20260503.laz` | 4.305.484.789 B | 508.809.556 | ETRS89 / UTM 29N |
| Majadas TLS | `SensOFOREST_2026_02_26_Majadas_pointcloud_TLS_20260503.laz` | 1.343.266.470 B | 415.465.882 | ETRS89 / UTM 30N |

El registro no declara una licencia en sus metadatos; se usa la copia solo para pruebas locales, sin
redistribuirla. El README y los archivos del [registro](https://zenodo.org/records/20000277) describen
los sensores, fechas y zonas UTM. Los conteos se leen de la cabecera LAS de cada archivo. Los checksums MD5
publicados por el registro y verificados en ambas copias son: Albuera
`71ff22c046d6eb83c371791f680bea13`; Majadas `fd0b4433e805246f2442df037dfadb6b`.

Para medir la conversión se puede ejecutar, con un destino `.oct` nuevo por archivo:

```sh
cargo run --release -p viewer_convert --example real_dataset_bench -- convert \
  data/datasets/sensoforest/SensOFOREST_2026_02_22_Albuera_Pointcloud_UAV_LiDAR_20260503.laz \
  data/datasets/sensoforest/albuera-uav.oct
cargo run --release -p viewer_convert --example real_dataset_bench -- convert \
  data/datasets/sensoforest/SensOFOREST_2026_02_26_Majadas_pointcloud_TLS_20260503.laz \
  data/datasets/sensoforest/majadas-tls.oct
```

La conversión completa y las mediciones de tiempo/RSS no se ejecutan al preparar los datos: son los
tests de estrés posteriores. Antes de convertir, revisa espacio libre y configura el destino local;
los archivos `.oct` resultantes también están ignorados por Git.

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
