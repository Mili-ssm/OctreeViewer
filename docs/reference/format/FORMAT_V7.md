# Formato nativo `.oct`, v7 (final)

Identidad: cabecera mayor **7**, revisión **0**, texto `v7`. Es la identidad final que congela
el [ADR 0044](../../adr/0044-v7-final-congelada-y-predeterminada.md) sobre la disposición de bytes
de [v7.5-pre](FORMAT_V7_PRE5.md). Esta página solo recoge lo que cambia respecto a esa
especificación; todo lo demás (marco global, CRS, páginas, códecs, CRC, límites) es idéntico.

## Identidad y diferencias con v7.5-pre

| Campo | v7.5-pre | v7 |
| --- | --- | --- |
| Revisión de cabecera (`.oct` y `.octattrs`) | `5` | `0` (con su CRC) |
| Magic del índice | `OCTVI75P` | `OCTVIDX7` |
| Magic del pie (ambos ficheros) | `OCTVE75P` | `OCTVEND7` |
| CRC del índice y del pie | según sus bytes | cambian solo por los magics |
| Capacidades (páginas, marco global, códecs, planos de bytes) | las de v7.5-pre | las mismas |
| Checkpoints por bloques | no | no |
| Monolítico | no: pareja `.oct` + `.octattrs` | no: pareja `.oct` + `.octattrs` |

`crates/viewer_format/tests/v7_final.rs` comprueba que, con la misma entrada y el mismo orden de
filas, los ficheros de v7 y v7.5-pre difieren solo en esos bytes.

## Orden de filas

- El lector admite cualquier orden de filas por bloque ([ADR 0041](../../adr/0041-orden-de-filas-libre-por-bloque.md)).
- El escritor de v7 usa siempre el orden híbrido ([ADR 0044](../../adr/0044-v7-final-congelada-y-predeterminada.md)).
- El híbrido sobre `v7.5-pre` existe solo para medir ([ADR 0042](../../adr/0042-escritor-hibrido-en-la-identidad-final-de-v7.md)).

## Lectores

- El lector síncrono (`FormatReader::new`) compara la identidad de la cabecera con la del pie y
  rechaza un fichero cuyo pie sea de otra identidad (`InvalidFooter`).
- Los lectores perezosos (`open_lazy`, `paged_external`, el lector asíncrono de wasm) no leen la
  cabecera: se orientan por el pie y los magics del índice. Un fichero con cabecera de una identidad
  y pie de otra solo lo detecta el lector síncrono; los CRC de cabecera, índice y pie siguen
  validándose donde se leen.

## Límites del escritor

El escritor rechaza esquemas de más de 256 atributos en todas las versiones escribibles
(`FormatError::TooManyAttributes`, RF-27, revisión 30-10-2026 del
[ADR 0007](../../adr/0007-limites-por-defecto-del-lector.md)): ningún fichero escrito necesita
límites de lector explícitos por su número de atributos (el número de nodos y de bloques sigue
sujeto a los límites por defecto del lector).
