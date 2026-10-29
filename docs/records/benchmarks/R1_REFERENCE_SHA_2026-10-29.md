# Referencias SHA del CLI tras la norma portátil (R1, 29-10-2026)

Registro fechado e inmutable. Sustituye a las referencias SHA de [WEB_CONVERTER_BASELINE_2026-10-29](WEB_CONVERTER_BASELINE_2026-10-29.md) (M0) para `.oct`/`.octattrs`; los tiempos de M0 no cambian. Motivo: [ADR 0016](../../adr/0016-norma-portatil-en-el-error-de-cuantizacion.md) (`f64::hypot` sustituido por `norm3`). Datos crudos: [JSON](data/r1-reference-sha-2026-10-29.json) (SHA completos y comparación por caso). Los ficheros intermedios (`target/r1/`) son efímeros y se borraron.

## Entorno

Linux 7.2.11-200.fc44, glibc 2.43; `rustc 1.101.0-nightly (41089d07a 2026-10-27)`; perfil `perf`; CLI con opciones por defecto salvo indicación (V5, 4 workers, 262144 puntos/bloque, error 0,01). Binario nuevo: commit `c442fb9` (norma portátil); binario antiguo: compilado de `22ab9ab` (con `hypot`), usado para comparar bloque a bloque. No se midió ningún tiempo.

## Comandos

```sh
cargo build --profile perf -p viewer_convert --bins --examples
target/perf/viewer_convert INPUT OUT.oct [--points-per-block 4096] [--spatial-divisions 2|8]
sha256sum OUT.oct OUT.octattrs
```

Entradas: `data/samples/fs_e430_n4008_18.las`; `autzen.copc.laz` (de `las-0.11.2/tests/data`); Belvedere (`data/datasets/belv_2009_digital-aerial_pcd_copc.laz`); Majadas (`data/datasets/sensoforest/SensOFOREST_2026_02_26_Majadas_pointcloud_TLS_20260503.laz`); `tools/laz_chunk_probe/fixtures/synthetic_variable_chunks.laz`.

## Referencias nuevas

SHA-256 completos en el JSON. Los SHA «antiguos» son los del binario con `hypot` en este equipo (glibc 2.43); coinciden con M0 en los cinco casos que M0 también midió (LAS pequeño, autzen, Belvedere x2, Majadas).

| Caso | `.oct` nuevo | `.oct` antiguo | `.octattrs` nuevo | `.octattrs` antiguo | Bloques | Bloques con `max_error` distinto | Bytes distintos del índice |
| --- | --- | --- | --- | --- | ---: | ---: | ---: |
| LAS pequeño, defecto | `176ee42de533ca13…` | `413858a908b4794b…` | `ee1e5daa735045db…` | `964c89effe7a94ff…` | 1 | 1 | 2 |
| LAS pequeño, 4096 x div. 2 | `8edbcce23f413faf…` | `980dc539125bba13…` | `da0a82d869603f44…` | `7b8b52a8eb21d9e7…` | 91 | 38 | 69 |
| LAS pequeño, 4096 x div. 8 | `d0be2da963d8f77f…` | `a5c4df10daebe77b…` | `b1cc49a572df6335…` | `01f5a2d99c065972…` | 550 | 266 | 528 |
| COPC pequeño (autzen) | `6fdd7b0e725f0567…` | `5966be999f77397f…` | `e1f7ad1193b85a36…` | `3be1310bcf3b3488…` | 1 | 1 | 2 |
| Belvedere, defecto | `ecdc4889778c082e…` | `6ca0f86d376c79d0…` | `cd3cad4571b24743…` | `0afa9c6041a6ae3f…` | 1.343 | 740 | 1.381 |
| Belvedere, divisiones 8 | `3aee289064d51617…` | `fb90a490f9884239…` | `816959b4d1aa3491…` | `b5124e84f7c4e12a…` | 5.140 | 2.914 | 5.822 |
| Majadas | `38168777e6ac8356…` | `742262867c844bb4…` | `6f968f11c330238a…` | `d45cd5fd70816d24…` | 6.259 | 1.977 | 3.519 |
| Sintético 4096 x div. 2 | `0b8a8016cf13f4e2…` | `22ba7b4e6a63969c…` | `c8e7008c70e4d5d8…` | `4a7b92dba322561b…` | 21 | 6 | 14 |
| Sintético 4096 x div. 8 | `657f70173571ee99…` | `af0aaa75ebe724a3…` | `d8478842254c1164…` | `34009eb97e0a9d40…` | 65 | 24 | 50 |

Los casos Belvedere con 1 y 8 workers de M0 no se repitieron: la independencia del número de workers se comprobó en M0 y la norma no depende de ellos.

## Qué difiere respecto a M0

En los nueve casos, antiguo y nuevo tienen el mismo tamaño, el mismo desplazamiento y longitud del índice, y **la región de payloads de bloques (desde la cabecera hasta el inicio del índice) es idéntica byte a byte**. Cambian solo:

- el `quantization_error` (`f64`) de algunos bloques, en 1-2 bytes bajos de la mantisa (columna «Bloques con `max_error` distinto»; se compararon los 8 bytes del campo en cada entrada de la tabla de bloques del índice);
- el `geometric_error` de nodos, que suma el error máximo de sus bloques (`writer.rs`: `node.geometric_error() + block_error`); explica los bytes distintos del índice que quedan fuera de los campos de bloque (**inferido** por la fórmula, no aislado nodo a nodo);
- los CRC del índice y del pie (no contados en la última columna), y por tanto el `.oct` completo;
- el `.octattrs` (otro SHA aunque su contenido de atributos no cambia; **inferido**: referencia la identidad/CRC del `.oct`; no se comprobó campo a campo).

Los puntos decodificados no se compararon uno a uno: la igualdad de la región de payloads, que contiene todas las coordenadas cuantizadas, lo hace innecesario.

## Paridad con wasm32

`tools/convert_parity/run.mjs` (sin casos exentos): LAS pequeño por defecto, 4096 x div. 2 y 8, sintético 4096 x div. 2 y 8, y autzen 4096 x div. 2 coinciden nativo frente a wasm32 en SHA-256 de `.oct` y `.octattrs` y en número de bloques. Belvedere y Majadas no se ejecutaron en wasm32.

## Límites

- Una sola ejecución por caso; sin tiempos (no es un benchmark).
- Los SHA valen para este formato (V5) y estas opciones; los ficheros versionados de `data/samples/*.oct` no se regeneraron (ninguna prueba los compara byte a byte) y conservan el `max_error` antiguo, igualmente válido.
- La ruta de entrada EPT (`viewer_convert/src/ept.rs`, `node_diagonal`) aún usa `hypot`; es solo nativa y no entra en estas referencias.
