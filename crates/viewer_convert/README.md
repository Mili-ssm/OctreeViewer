# viewer_convert

Shell nativo de la conversión LAS/LAZ a la pareja `.oct`/`.octattrs`: ficheros, hilos,
spools acotados y publicación validada (EPT y checkpoints se retiraron con la V8, ADR 0061). La lógica pura vive en
[`viewer_convert_core`](../viewer_convert_core/README.md).
Entrada: binario `viewer_convert INPUT OUTPUT.oct --format-version …` y `convert_file*` con
`ConvertOptions`. Uso: [uso y formato](../../docs/estado.md#uso-y-formato); límites:
[inventario](../../docs/reference/inventory.md#límites).
