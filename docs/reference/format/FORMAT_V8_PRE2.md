# Formato nativo `.oct`, v8.2-pre

> Histórico: formato retirado con la V8 (D4, 21-11-2026); la referencia vigente es [FORMAT_V8](FORMAT_V8.md).

Identidad de desarrollo: cabecera mayor **8**, revisión **2**, texto `v8.2-pre`; opt-in
(`viewer_convert --format-version v8.2-pre`). La salida predeterminada sigue siendo [v7](FORMAT_V7.md). Decisiones
en el [ADR 0058](../../adr/0058-formato-v8-experimental.md); plan en [formato-v8-pre2](../../plans/formato-v8-pre2.md).

Es lo mismo que [v8.1-pre](FORMAT_V8_PRE1.md) más el códec zstd para los payloads de `.octattrs`. Con
`--attr-codec lz4` (por defecto) un fichero `v8.2-pre` es byte a byte uno `v8.1-pre` salvo la identidad. Es
experimental y **no está congelada** (ADR 0058, D6): puede cambiar sin compatibilidad. Tras medirla, la V8 se queda solo con LZ4
([ADR 0060](../../adr/0060-v8-solo-lz4.md)), así que esta revisión no pasa a la V8 congelada. Medida de los cuatro esquemas:
[registro del 20-11-2026](../../records/benchmarks/FORMATO_V8_PRE2_20-11-2026.md).

## Identidad

| Campo | v8.1-pre | v8.2-pre |
| --- | --- | --- |
| Revisión de cabecera (`.oct` y `.octattrs`) | `1` (mayor `8`) | `2` (mayor `8`) |
| Magic del índice | `OCTVI81P` | `OCTVI82P` |
| Magic del pie (ambos ficheros) | `OCTVE81P` | `OCTVE82P` |
| Capacidades (páginas, marco global, planos de bytes, filas híbridas fijas, color derivado, columnas LOD, cuantiles) | las de v8.1-pre | las mismas |
| Códec zstd | no | sí, solo en `.octattrs` |

## Códecs de payload

| Etiqueta (`BlockCodec`) | Valor | Contenido | Dónde |
| --- | ---: | --- | --- |
| `Zstd` | 5 | Una trama zstd | Payloads de `.octattrs` |
| `BytePlanesZstd` | 6 | Planos de bytes independientes de una tupla de atributo, comprimidos con zstd | Payloads de `.octattrs` |
| Diccionario, etiqueta interna `2` | — | Parte del diccionario en zstd (las etiquetas internas 0 y 1 siguen siendo crudo y LZ4) | Payloads de `.octattrs` |

- **Trama:** nivel 3, sin número mágico, sin checksum y sin tamaño de contenido (el directorio ya guarda
  `raw_length`). El lector descomprime en un búfer de exactamente `raw_length` bytes; otro tamaño es un error.
- **Fuera de `.octattrs`:** la geometría de `.oct`, el índice y los registros de checkpoint nunca usan zstd, y un
  layout anterior a v8.2-pre que traiga las etiquetas 5, 6 o la interna 2 es un fichero mal formado.
- **Memoria:** el plan de lectura reserva 1 MiB de contexto de descompresión por payload zstd, también en diccionarios.
- **Elección:** como en LZ4, el escritor prueba los candidatos (crudo, constante, diccionario, planos) y guarda el más
  pequeño; zstd sustituye a LZ4 como etapa de entropía de todos los candidatos de una columna.

## Esquemas del conversor (`--attr-codec`)

Opción experimental, solo con `--format-version v8.2-pre` (con otro formato, un esquema distinto de `lz4` es un error), que decide qué columnas usan zstd; el fichero no la guarda y cualquier lector v8.2-pre lee todos
los esquemas. Al congelar la V8 se fija un esquema y la opción desaparece (D4).

| Esquema | Columnas con zstd |
| --- | --- |
| `lz4` (por defecto) | Ninguna |
| `cold` | Frías: `return_number`, `number_of_returns`, `scan_angle_degrees`, `user_data`, `point_source_id`, `gps_time`, `nir`, `las_flags`, las `extra:` y cualquier nombre desconocido |
| `cold-rgb` | Frías y `rgb` (la fuente del color de pantalla de las hojas) |
| `all` | Todas las de `.octattrs`, también intensidad, clase y el color de pantalla de LOD |

Las clases están en `crates/viewer_format/src/attribute_scheme.rs`. El conversor web acepta lo mismo con la opción
`attrCodec`, y su salida es byte a byte igual a la del CLI.
