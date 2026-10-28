# viewer_format

Formato `.oct`/`.octattrs` versionado sobre streams `Read + Seek`/`Write + Seek`.
Entrada: `FormatReader` (lectura selectiva), `FormatWriter` (salida incremental) y la política de
versiones en `FormatVersion` (`DEFAULT_OUTPUT`, `WRITABLE`, `supports_block_checkpoint()`).
Features apagadas por defecto: `experimental`, `async-reader` (`AsyncFormatReader`, no-wasm).
Especificaciones: [FORMAT_V5](../../docs/reference/format/FORMAT_V5.md) y
[V7](../../docs/README.md#referencia).
