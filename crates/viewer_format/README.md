# viewer_format

Formato `.oct`/`.octattrs` versionado sobre streams `Read + Seek`/`Write + Seek`.
Entrada: `FormatReader` (lectura selectiva), `FormatWriter` (salida incremental) y la política de
versiones en `FormatVersion` (`DEFAULT_OUTPUT`, `WRITABLE`).
Features: [inventario](../../docs/reference/inventory.md#componentes-y-puntos-de-entrada).
Solo lee y escribe V8 (los demás formatos dan `RetiredFormat`, «reconvierte»). Especificación vigente:
[FORMAT_V8](../../docs/reference/format/FORMAT_V8.md); las anteriores son históricas ([índice](../../docs/README.md#referencia)).
