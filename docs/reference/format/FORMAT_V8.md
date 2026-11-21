# Formato nativo `.oct`, V8 (vigente)

**Es el único formato que el visor, el conversor y `verify` leen y escriben** (D4, 21-11-2026). Identidad: cabecera
mayor **8**, revisión **0**, texto `v8`; magic del índice `OCTVIDX8`, magic del pie `OCTVEND8`. Decisiones:
[ADR 0058](../../adr/0058-formato-v8-experimental.md) (V8 y sus revisiones previas),
[ADR 0060](../../adr/0060-v8-solo-lz4.md) (solo LZ4, sin zstd) y
[ADR 0061](../../adr/0061-ept-y-checkpoints-se-retiran-con-la-v8.md) (sin EPT ni checkpoints); diseño en
[formato-v8](../../records/archive/formato-v8.md) y plan de congelado en [formato-v8-congelado](../../records/archive/formato-v8-congelado.md).
Fuente de verdad: `crates/viewer_format/src/` (`version.rs`, `layout.rs`, `page_codec.rs`, `paged_index.rs`,
`global_frame.rs`, `codec.rs`, `attribute_codec.rs`, `retired.rs`, `prepare.rs`).

Los bytes son los de la revisión previa [v8.1-pre](FORMAT_V8_PRE1.md) salvo la revisión de cabecera y los magics; el
resto de formatos están retirados y solo se conservan como [históricos](#formatos-históricos). Todo número es
little-endian; los CRC son CRC-32 IEEE reflejado (detectan corrupción accidental, no manipulación).

## Resumen

| Aspecto | Valor |
| --- | --- |
| Ficheros | Pareja `.oct` (geometría + jerarquía) y `.octattrs` (color y atributos); no hay monolítico ([ADR 0001](../../adr/0001-pareja-separada-oct-octattrs.md)). El `.oct` se abre solo, con color neutro |
| Cabecera | 24 B: magic `OCTVFRM1`, mayor `8`, longitud `24`, rol (`1` `.oct`, `2` `.octattrs`), revisión `0`, CRC de los 20 B previos |
| Pie | 40 B, magic `OCTVEND8`: desplazamiento y longitud del índice, longitud del fichero, CRC del índice y CRC del pie |
| Marco | Retícula global implícita en los bits de `q = round(c × 2^15)`; nodo = `(nivel, prefijo)` |
| Índice | Paginado: bloque de metadatos + página raíz junto al pie; resto en páginas hijas |
| Bloques | 64 000 puntos por hoja y 2 divisiones espaciales por eje, **fijos** (sin opciones); filas en orden híbrido |
| Códecs | Crudo, LZ4, constante, diccionario, planos de bytes; geometría cruda, `DeltaVarint` u ocupación compacta |
| No existe | zstd, checkpoints, EPT, monolítico, otras divisiones o tamaños de bloque |

## Estructura de los ficheros

```text
.oct       cabecera 24 B | geometría de los bloques | páginas hijas (por niveles) | bloque de metadatos + página raíz | pie 40 B
.octattrs  cabecera 24 B | color y columnas de cada bloque (en orden de escritura) | pie 40 B
```

- **Pie:** `OCTVEND8`, `index_offset u64`, `index_length u64`, `file_length u64`, CRC del índice `u32`, CRC del pie `u32`
  (sobre los 36 B previos). `index_offset + index_length` es el inicio del pie. El pie del `.octattrs` repite la misma
  forma: desplazamiento, longitud y CRC del índice **del `.oct`** (identifica la pareja) y su propia longitud.
- **Cola:** la página raíz es lo último del índice, así que una lectura de la cola (`TAIL_READ_BYTES` = 1 MiB, o el
  fichero entero si es menor) trae pie, metadatos y raíz. Los lectores perezosos identifican la versión por el magic del
  pie y no leen la cabecera; solo hace falta una segunda petición si el índice no cabe en la cola.
- **Pareja:** el lector rechaza un `.oct` y un `.octattrs` de identidad distinta (también uno V8 con otro retirado).
- **Contigüidad:** las secciones de geometría llenan `[24, pages_start)` sin huecos ni solapes, y las de atributos llenan
  el `.octattrs` entre cabecera y pie.

## Marco global y retícula

- **Cuanto:** `q = round(c × 2^15)` como `i64` (`UNITS_PER_QUANTUM` = 2⁻¹⁵); el cuanto lo fija la versión, no es un
  campo. Se rechaza un punto con `|c| ≥ 2^48`; el conversor, `|c| ≥ 2^35` para que las cotas `f64` de los nodos sean
  exactas.
- **Retícula:** `u = q − base` leído como `u64`. Con CRS identificado (`autoridad` + código, o bytes crudos), `base = i64::MIN`
  (retícula **global**, común a toda nube del CRS, RF-23). Una nube **local** (sin código ni CRS crudo) usa como `base`
  su esquina mínima y no se mezcla con otras.
- **Nodo** a nivel absoluto `a` (0..=64) con prefijo `P` (`a` bits por eje): cubre `u ∈ [P << (64−a), (P+1) << (64−a))`;
  lado = 2^(49−a) unidades. Hijo: `P_hijo = (P << 1) | bit` por eje. Octante = `x | y<<1 | z<<2`, hijos por octante creciente.
- **Raíz almacenada:** el nodo más pequeño que contiene todos los puntos; la cadena de nodos de un solo hijo por encima se
  omite. El índice guarda su nivel `a0` y prefijo.
- **Origen del marco y extensión:** el origen (3 × `i64` cuantos) es un punto dentro de la raíz; el máximo de los datos
  (3 × `i64`) va justo después. `[origen, máximo]` son los límites del dataset; las cotas de nodo son cubos de la retícula.
  Los `f64` de origen son derivados e informativos. Las posiciones se decodifican como
  `(((origen_bloque + desplazamiento) − origen_marco) × 2^-15) as f32`.
- **Registro del CRS:** autoridad (`u32` longitud + ASCII en mayúsculas, ≤ 16 B, vacía si no hay código), código `u32`
  (`0` si no hay autoridad), codificación cruda `u8` (`0` ninguna, `1` WKT, `2` claves GeoTIFF) y bytes crudos (vacíos si y
  solo si la codificación es `0`); tope `max_string_bytes` (1 MiB).
- **Escala y desplazamiento de la fuente:** `u8` presente + 3 × `f64` escala (> 0) + 3 × `f64` desplazamiento; permiten
  exportar el entero LAS exacto si la escala es ≥ 1e-4.

## Índice paginado

### Bloque de metadatos

Campos en orden: magic `OCTVIDX8`; origen del marco; nivel `a0`, prefijo de la raíz, origen en cuantos y máximo de los
datos; recuento de puntos de la fuente; registro del CRS; escala y desplazamiento; política de muestreo (texto
descriptivo `sample-grid:256;leaf:64000`); semántica (`0` reemplazo; el lector también admite `1` aditiva, pero el escritor la rechaza en los diseños paginados); escala visual; esquema de atributos; después:

| Campo | Tipo | Regla |
| --- | --- | --- |
| Total de nodos y de bloques | `u64` × 2 | ≤ `max_nodes` / `max_blocks`; informativos hasta leer todas las páginas |
| Inicio de páginas | `u64` | ≥ 24 y ≤ `index_offset` |
| Espaciado de referencia | `f64` | finito y > 0: mediana ponderada por puntos del espaciado de las hojas |
| Cuantiles de distancia (D5) | 3 × `f64` | `p1 ≤ p5 ≤ p50`, finitos, no negativos; todo ceros = «sin medida» |
| Columnas LOD (D2) | `u32` n + n × `u32` | índices del esquema que conservan los LOD, estrictamente crecientes |
| Origen del color de pantalla | `u8` + `u32` | `0,0` ninguno; `1,i` RGB `u16` × 3; `2,i` intensidad `u16`; otro par es error |
| Rangos globales de atributos | por atributo y componente | `u8` presente, `f64` mín y máx |
| Extensión | `u32` longitud | `0` o `2 + 108 × n` (Extra Bytes de LAS, [abajo](#flags-las-y-extra-bytes)) |

Esquema: como máximo 256 atributos (`TooManyAttributes`, RF-27). Los cuantiles se calculan de una muestra con semilla
fija en las hojas (`SAMPLE_SEED`, en `crates/viewer_convert_core/src/pipeline/nn_distance.rs`); las columnas LOD son `intensity` y `classification` si existen; el origen del color es
`rgb` de 16 bits si existe, si no `intensity` de 16 bits, si no ninguno.

### Páginas y nodos

Una página cubre `k` niveles (1 ≤ `k` ≤ 5, `MAX_PAGE_LEVELS`) bajo su **ancla**; el registro del ancla vive en la página
padre. El escritor elige el mayor `k` cuya página codificada cabe en 512 KiB (`TARGET_PAGE_BYTES`).

```text
página := cabecera (38 B: nivel del ancla u8, prefijo 3 × u64, k u8, node_count u32, first_block_id u64),
          registros de nodo × node_count, CRC-32 de todo lo anterior
```

- **Orden:** `(profundidad relativa, ruta)` ascendente; la ruta son `3r` bits de octantes (`r` ≤ 5). El nodo raíz es el primer
  registro de la página 0 (profundidad 0).
- **`NodeId`** derivado, nunca guardado: `número de página << 32 | índice` (`page_node_id`, `split_node_id`); depende del
  plan de páginas, así que no se persiste entre ficheros. Las páginas se numeran por niveles (raíz = 0).
- **`BlockId`** no se guarda: los bloques de una página son `first_block_id`, `+1`, … en orden de nodo y de bloque.
- **Registro de nodo:** profundidad `u8`; ruta `u16`; banderas `u8` (bit 0 = tiene hijos, resto 0); puntos `u64` (suma de
  sus bloques); error geométrico `f64` (≥ 0 y ≥ el de cada bloque); exponente de espaciado `i8` **solo en hojas** (espaciado
  `2^e`, `−15 ≤ e ≤ 49 − nivel`; un nodo con hijos usa `2^(41 − nivel)`); recuento de bloques `u32`; bloques; rangos por
  nodo (`u16` n, índice `u16` ascendente, mín y máx); puntero a la página hija (`u64` desplazamiento, `u32` longitud,
  `u32` número de página ≥ 1) solo si la profundidad es `k` y tiene hijos.
- **Registro de bloque:** desplazamiento de geometría, desplazamiento de atributos y puntos (`u64` × 3); codificación de
  geometría `u8`; error de cuantización `f64`; celda 3 × `u32` solo si el nivel del nodo es < 32 (nodo partido: origen =
  origen del nodo + `celda << 32`); una entrada por flujo con códec `u8`, CRC `u32` y longitud almacenada `u64` (la
  geometría añade su longitud sin comprimir).
- **Validación por página** (al leerla): longitud y CRC; cabecera (nivel ≤ 64, `k` 1..=5, `node_count` ≤ 37 448 (+1 en la raíz)
  y ≤ `max_nodes`); ancla igual a la del puntero seguido; claves estrictamente ascendentes; padres presentes en la página;
  bloques dentro de `[24, pages_start)` sin solapes; punteros dentro de `[pages_start, index_offset)`; ningún `BlockId`
  repetido. Las comprobaciones globales (sin huecos, totales, un propietario por bloque) son de `viewer_convert verify` y
  del lector `FormatReader`, que lee todas las páginas.
- **Límites por defecto del lector** (`FormatReaderLimits`): índice 256 MiB, bloque 256 MiB, página 1 MiB, 4096 páginas
  residentes, 2 000 000 nodos y bloques, 1 MiB por cadena, 65 535 componentes, 2²³ rangos de atributos.

### Flags LAS y Extra Bytes

El esquema de una conversión LAS incluye siempre `las_flags` (`u8`: bit 0 sintético, 1 punto clave, 2 retenido,
3 solapado) y una columna `extra:<nombre>` por descriptor de Extra Byte (tipos 1–30; 0 y 31–255 son error). La extensión
del bloque de metadatos guarda por columna 108 B: nombre (32 B), tipo LAS, opciones, reservado, *no data* (3 × 8 B), escala
(3 × `f64`) y desplazamiento (3 × `f64`). Valor real = crudo × escala + desplazamiento.

## Bloques

Un bloque es **LOD** si su nodo tiene hijos y **hoja** si no. Tiene `2 + n` flujos: 0 geometría (`.oct`), 1 color de
pantalla (RGB de 8 bits) y `2 + i` por columna del esquema (`.octattrs`). La regla única es `StorageLayout::stored_column`
(`layout.rs`), que consultan escritor, códecs, lector y `verify`:

| Flujo | Hoja | LOD |
| --- | --- | --- |
| 0 geometría | guardado | guardado |
| 1 color de pantalla | **derivado** de la columna de origen si hay origen; guardado si no | guardado (media de 8 bits de la celda) |
| `2 + i` columna del esquema | guardada | guardada solo si `i` está en las columnas LOD; **ausente** si no |

- Un flujo derivado o ausente no tiene entrada en el directorio ni ocupa bytes. Los LOD guardan, pues, solo geometría,
  color de pantalla, intensidad y clasificación.
- **Color derivado:** `valor >> 8` por canal de la columna de origen (RGB, o intensidad repetida en r, g y b); el escritor
  comprueba cada hoja contra esa derivación y falla si difiere.
- **Orden de filas:** el conversor fija siempre el híbrido (`FormatVersion::fixed_row_order`; la API del escritor usa por defecto el orden de la fuente) ([ADR 0044](../../adr/0044-v7-final-congelada-y-predeterminada.md));
  el lector admite cualquier orden ([ADR 0041](../../adr/0041-orden-de-filas-libre-por-bloque.md)).
- **Tamaños:** 64 000 puntos por hoja y 2 divisiones por eje, fijos; un bloque por encima de `--max-error` se rechaza.

## Códecs

| `BlockCodec` | Valor | Contenido |
| --- | ---: | --- |
| `None` | 0 | Bytes crudos; longitud almacenada = longitud sin comprimir |
| `Lz4` | 1 | Bloque LZ4 (`lz4_flex`) |
| `Constant` | 2 | Una tupla completa repetida en todas las filas |
| `Dictionary` | 3 | Paleta local de tuplas e ids de fila empaquetados en bits (cabecera de 10 B; las partes se guardan crudas o en LZ4) |
| `BytePlanes` | 4 | LZ4 sobre planos de bytes independientes de una tupla de atributo |

Cualquier otro valor (también 5 y 6, de zstd) es `UnsupportedCodec` o fichero mal formado. El escritor prueba los
candidatos de cada columna y guarda el más pequeño; el lector descomprime en un búfer de exactamente `raw_length` y
comprueba el CRC antes de expandir. La **geometría** son 3 × `u32` por fila (`offset = q − origen_bloque`, `0 ≤ offset < 2³²`)
con codificación `RawU32` (0), `DeltaVarint` (1) u ocupación compacta de 24 bits (3, `CompactOccupied`); la etiqueta 2
(`Occupied`) es de revisiones previas.

## Identidad, formatos retirados y errores

`FormatVersion` solo conoce `V8`; cualquier otra pareja `(mayor, revisión)` es `Unknown`. Un fichero de una familia retirada
falla al abrir con `FormatError::RetiredFormat`: *«this file is format X, retired with V8: reconvert it from the LAS/LAZ
source»* (`retired.rs`). Se reconoce por el magic del pie, o por mayor y revisión de la cabecera si el pie se perdió, y
nunca sobre una cabecera V8 intacta con un pie dañado (eso es corrupción del fichero V8).

| Familia | Magic del pie | Familia | Magic del pie |
| --- | --- | --- | --- |
| v5 | `OCTVEND5` | v7.4-pre | `OCTVE74P` |
| v6.1-pre | `OCTVE61P` | v7.5-pre | `OCTVE75P` |
| v6.2-pre | `OCTVE62P` | v7 | `OCTVEND7` |
| v6.3-pre | `OCTVE63P` | v8.1-pre | `OCTVE81P` |
| v6.4-pre | `OCTVE64P` | v8.2-pre | `OCTVE82P` |
| v7.1-pre | `OCTVE71P` | | |
| v7.2-pre | `OCTVE72P` | | |
| v7.3-pre | `OCTVE73P` | | |

Las 13 familias tienen además su par (mayor, revisión) de cabecera. El magic provisional `OCTVWFRM` da
`ProvisionalFormat` (regenerar desde LAS/LAZ). Fixtures de error en
[`crates/viewer_format/tests/fixtures/retired/`](../../../crates/viewer_format/tests/fixtures/retired/README.md).

## Formatos históricos

Retirados con la V8 (D4); no se leen, y explican los registros:
[V5](FORMAT_V5.md), [v7](FORMAT_V7.md), [v7.1-pre](FORMAT_V7_PRE1.md), [v7.2-pre](FORMAT_V7_PRE2.md),
[v7.3-pre](FORMAT_V7_PRE3.md), [v7.4-pre](FORMAT_V7_PRE4.md), [v7.5-pre](FORMAT_V7_PRE5.md),
[v8.1-pre](FORMAT_V8_PRE1.md) y [v8.2-pre](FORMAT_V8_PRE2.md); v6 en [`docs/records/archive/`](../../records/archive/FORMAT_V6_PRE1.md).
