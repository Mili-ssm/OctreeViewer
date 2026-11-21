# Formato nativo `.oct`, v8.1-pre

> Histórico: formato retirado con la V8 (D4, 21-11-2026); la referencia vigente es [FORMAT_V8](FORMAT_V8.md).

Identidad de desarrollo: cabecera mayor **8**, revisión **1**, texto `v8.1-pre`; opt-in
(`viewer_convert --format-version v8.1-pre`). La salida predeterminada sigue siendo [v7](FORMAT_V7.md). Decisiones
en el [ADR 0058](../../adr/0058-formato-v8-experimental.md); diseño en [formato-v8](../../plans/formato-v8.md), plan en
[formato-v8-pre1](../../plans/formato-v8-pre1.md) y medida en el
[registro](../../records/benchmarks/FORMATO_V8_PRE1_20-11-2026.md). Es experimental y **no está congelada** (ADR 0058,
D6): puede cambiar en revisiones previas posteriores sin compatibilidad.

Es la disposición de v7 (páginas, marco global, códecs, planos de bytes, CRC) con tres cambios: tres campos nuevos en
la cabecera del índice y dos reglas de qué columnas guarda cada bloque. Todo número es little-endian, como en v7.

## Identidad y diferencias con v7

| Campo | v7 | v8.1-pre |
| --- | --- | --- |
| Revisión de cabecera (`.oct` y `.octattrs`) | `0` | `1` (mayor `8`) |
| Magic del índice | `OCTVIDX7` | `OCTVI81P` |
| Magic del pie (ambos ficheros) | `OCTVEND7` | `OCTVE81P` |
| Capacidades (páginas, marco global, códecs, planos de bytes, filas híbridas fijas) | las de v7 | las mismas |
| Cuantiles de distancia, lista de columnas LOD y origen del color de pantalla en la cabecera del índice | no | sí |
| Columnas que guarda cada bloque | todas | según las reglas de más abajo |
| Checkpoints por bloques | no | no |
| Monolítico | no: pareja `.oct` + `.octattrs` | no: pareja `.oct` + `.octattrs` |

## Campos nuevos de la cabecera del índice

Van en este orden, justo después del espaciado de referencia (`f64`) y antes de los rangos globales de atributos
(`parse_header_block` y su escritor en `crates/viewer_format/src/paged_index.rs`). Los tres campos son obligatorios en
v8.1-pre y están prohibidos en las demás revisiones: el escritor rechaza unos metadatos que no lo cumplan.

| Campo | Bytes | Contenido | Validación |
| --- | --- | --- | --- |
| Cuantiles de distancia (D5) | 3 × `f64` | `p1`, `p5`, `p50` de la distancia al vecino más cercano, en unidades del dataset | finitos, no negativos y `p1 ≤ p5 ≤ p50`. Todo ceros significa «sin medida»: no hay tamaño mínimo recomendado (`p5 == 0`) |
| Columnas LOD (D2) | `u32` n + n × `u32` | índices del esquema de atributos que conservan los bloques LOD | n ≤ tamaño del esquema; índices estrictamente crecientes y dentro del esquema (una sola forma de bytes por conjunto) |
| Origen del color de pantalla | `u8` tipo + `u32` índice | `0, 0` ninguno; `1, i` RGB de 16 bits en la columna `i`; `2, i` intensidad de 16 bits en la columna `i`; cualquier otro par es un error de formato | la columna `i` debe ser `u16` de 3 componentes (tipo 1) o de 1 (tipo 2) |

Qué escribe el conversor:

- **Cuantiles:** de una muestra con semilla fija (`SAMPLE_SEED`, unos 2¹⁶ puntos: es un objetivo de la tasa de muestreo, no un tope) tomada en las hojas; cada
  hoja busca el vecino en su propia hoja, y las muestras se juntan en orden de nodo. El borde de hoja sobreestima algo.
- **Columnas LOD:** las columnas del esquema llamadas `intensity` y `classification`, las que existan (el color de
  pantalla de los LOD no es una columna del esquema: es el flujo 1 y se guarda siempre).
- **Origen del color:** `rgb` de 16 bits y 3 componentes si existe; si no, `intensity` de 16 bits; si no, ninguno (las
  hojas guardan entonces el color de pantalla).

## Reglas de almacenamiento por bloque

Un bloque es **LOD** si su nodo tiene hijos y **hoja** si no. Un bloque tiene `2 + n` flujos: 0 geometría, 1 color de pantalla (RGB de 8 bits) y uno por columna del esquema (`2 + índice`); la geometría va en el `.oct` y
los demás flujos, en el `.octattrs`. La regla única es `StorageLayout::stored_column` (`crates/viewer_format/src/layout.rs`), que consultan
escritor, códecs, lector y `verify`:

| Flujo | Hoja | LOD |
| --- | --- | --- |
| 0 geometría | guardado | guardado |
| 1 color de pantalla | **derivado** si hay origen (tipo 1 o 2); guardado si no hay | guardado (es la media de los 8 bits de la celda; no se deriva de nada) |
| `2 + i` columna del esquema | guardada | guardada si `i` está en la lista de columnas LOD; **ausente** si no |

- **Sin carga útil:** un flujo derivado o ausente no tiene registro en el directorio de cargas del bloque (ni códec, ni
  CRC, ni longitudes) y no ocupa bytes; su posición conserva el desplazamiento que tendría el siguiente flujo guardado.
- **Color derivado:** el lector reconstruye el color de pantalla de una hoja como `valor >> 8` por canal de la columna
  de origen (RGB: cada canal; intensidad: el mismo valor para r, g y b). El escritor comprueba cada hoja contra esa
  derivación y falla si difiere.
- **Columna ausente:** en un bloque LOD se lee como «no presente»; el visor, con un modo de color por atributo, dibuja el
  color de intensidad hasta que lleguen las hojas.
- **Número de nodos y bloques:** igual que en v7; el ahorro está en el `.octattrs` (ver el registro).

## Lectores y compatibilidad

Solo los lectores que conocen `OCTVI81P`/`OCTVE81P` abren v8.1-pre; el conversor (`verify`), el lector nativo y el
visor wasm lo leen. Es una revisión experimental, optativa y sin garantía de estabilidad ([ADR 0058](../../adr/0058-formato-v8-experimental.md)).
