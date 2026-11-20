# Formato nativo `.oct`, v8.2-pre

Identidad de desarrollo: cabecera mayor **8**, revisión **2**, texto `v8.2-pre`; opt-in
(`viewer_convert --format-version v8.2-pre`). La salida predeterminada sigue siendo [v7](FORMAT_V7.md). Decisiones
en el [ADR 0058](../../adr/0058-formato-v8-experimental.md); plan en [formato-v8-pre2](../../plans/formato-v8-pre2.md).

Es lo mismo que [v8.1-pre](FORMAT_V8_PRE1.md) más el códec zstd, que se añade en tareas posteriores. Hasta entonces un
fichero `v8.2-pre` es byte a byte uno `v8.1-pre` salvo la identidad. Es experimental y **no está congelada** (ADR 0058,
D6): puede cambiar sin compatibilidad.

## Identidad

| Campo | v8.1-pre | v8.2-pre |
| --- | --- | --- |
| Revisión de cabecera (`.oct` y `.octattrs`) | `1` (mayor `8`) | `2` (mayor `8`) |
| Magic del índice | `OCTVI81P` | `OCTVI82P` |
| Magic del pie (ambos ficheros) | `OCTVE81P` | `OCTVE82P` |
| Capacidades (páginas, marco global, planos de bytes, filas híbridas fijas, color derivado, columnas LOD, cuantiles) | las de v8.1-pre | las mismas |
| Códec zstd | no | sí (aún sin usar) |
