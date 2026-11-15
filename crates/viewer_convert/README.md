# viewer_convert

Shell nativo de la conversión LAS/LAZ y EPT local a la pareja `.oct`/`.octattrs`: ficheros, hilos,
spools acotados, checkpoints/resume y publicación validada. La lógica pura vive en
[`viewer_convert_core`](../viewer_convert_core/README.md).
Entrada: binario `viewer_convert INPUT OUTPUT.oct --format-version …` y `convert_file*` con
`ConvertOptions`. Uso: [uso y formato](../../docs/estado.md#uso-y-formato); límites:
[inventario](../../docs/reference/inventory.md#límites).
