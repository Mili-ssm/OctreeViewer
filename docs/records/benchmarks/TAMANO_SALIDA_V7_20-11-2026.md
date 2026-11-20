# Tamaño de la salida v7: de dónde salen los bytes — 20-11-2026

**Estado: medido.** Estudio pedido por el propietario el 20-11-2026, tras la fase B de temporales: el disco que
necesita una conversión lo marca ahora la salida (`.oct` + `.octattrs`), no los temporales
([registro de la fase B](TEMPORALES_CLI_FASE_B_20-11-2026.md), #109). Lleva a la decisión de diseñar una V8.

**Conclusión:** la salida de Albuera ocupa **3,66 × el LAZ** porque guarda **1,97 N puntos** (los LOD de la jerarquía
Replacement son otro 0,97 N, por el ×2 de densidad por nivel del [ADR 0020](../../adr/0020-piramide-lod-densa.md)) y
el **color dos veces**: el RGB de pantalla de 8 bits es el RGB de 16 bits `>> 8` (o la intensidad `>> 8` sin color,
`source_points.rs:140-148`) y se guarda aparte, casi sin comprimir. Las mayores reducciones cambian el formato v7,
congelado por el [ADR 0044](../../adr/0044-v7-final-congelada-y-predeterminada.md).

## Condiciones

`main` en `59b3d52`, binario `release`, opciones por defecto (`v7`), equipo del propietario. Tamaños por flujo leídos del
índice con `crates/viewer_format/examples/storage_payload_stats.rs`; ratios de otros códecs con programas desechables
sobre columnas decodificadas de una muestra de bloques (1/40 de los de Albuera, 1/8 de los de Belvedere, cada bloque
comprimido por separado) y con las CLI `zstd`/`xz` sobre tramos de 128 MB.

## Albuera (508 809 556 puntos de origen; LAZ 4 305 484 789 B)

`.oct` 4 563,2 MB + `.octattrs` 11 188,7 MB; puntos guardados 1 001 538 259 = **1,968 N**; índice y fijos ~7,8 MB.

| Flujo | Sin comprimir | Guardado | % de la carga | B por punto guardado | Codificación |
| --- | --- | --- | --- | --- | --- |
| Geometría | 6 078 MB | 4 555 MB | 28,9 | 4,55 | LZ4 991 MB; en crudo 3 564 MB (LZ4 no ayuda en la mayoría) |
| RGB de 16 bits | 6 009 MB | 3 726 MB | 23,7 | 3,72 | planos de bytes |
| Tiempo GPS (f64) | 8 012 MB | 3 009 MB | 19,1 | 3,00 | planos de bytes (37,6 % de lo crudo) |
| RGB de pantalla (8 bits) | 3 005 MB | 2 960 MB | 18,8 | 2,96 | en crudo 2 512 MB, LZ4 306 MB, planos 142 MB |
| Intensidad (u16) | 2 003 MB | 770 MB | 4,9 | 0,77 | diccionario |
| Ángulo de escaneo (f32) | 4 006 MB | 528 MB | 3,4 | 0,53 | diccionario |
| Retornos, id de fuente, clase | — | 196 MB | 1,2 | 0,20 | diccionario |
| Datos de usuario, flags | — | ~0 | 0 | ~0 | constantes |

## Belvedere (81 682 329 puntos de origen; LAZ 618,4 MB)

`.oct` 530,96 MB + `.octattrs` 889,64 MB = 2,30 × el LAZ; puntos guardados 1,626 N.

| Flujo | Guardado | % | B por punto guardado |
| --- | --- | --- | --- |
| Geometría | 529,8 MB | 37,3 | 3,99 |
| RGB de 16 bits | 499,4 MB | 35,2 | 3,76 |
| RGB de pantalla | 390,1 MB | 27,5 | 2,94 |
| Resto de atributos | < 0,1 MB | ~0 | constantes por bloque en esta nube |

Belvedere no representa los atributos: salvo el RGB, todos son constantes por bloque.

## Candidatos (estimaciones sobre Albuera, 15,74 GB; nada implementado)

| Candidato | Ahorro estimado | ¿Cambia el formato? | Coste o riesgo |
| --- | --- | --- | --- |
| Derivar el color de pantalla del RGB de 16 bits o de la intensidad al leer | −2,96 GB (−18,8 %); Belvedere −27,5 % | Sí | Sin pérdida; el lector o la subida a GPU hace `>> 8` |
| LOD solo con xyz, color, intensidad y clase | ~−3,65 GB (−23 %; supone que el coste por columna de una fila LOD es el de una hoja) | Sí | Se pierde el coloreado por atributo genérico en los niveles gruesos |
| LOD más ralo (×4 por nivel: LOD = N/3) | ~−32 % (×8: ~−43 %) | No en la disposición; revierte el ADR 0020 | Saltos de nivel más bruscos |
| LOD como referencias a filas de hoja | hasta ~−49 % | Sí, grande | Rompe la lectura independiente por nodo |
| zstd (nivel 3) sobre planos para tiempo GPS y RGB de 16 bits | −0,6 y −0,7 GB (−20 % y −19 % de esos flujos) | Sí (códec nuevo) | Codificación más lenta; el web necesita un decodificador zstd |
| Delta del tiempo GPS | sin ganancia (30,9 % frente a 29,9 % sin delta) | — | Las filas van en orden Morton, no temporal |
| Opción del conversor para omitir atributos (tiempo GPS, ángulo, retornos, id de fuente) | ~−3,7 GB (−23 %) | No | Con pérdida y a elección del usuario; el visor solo los usa en el coloreado genérico `attribute:N:M` |

Combinando el color derivado, los LOD sin atributos pesados y zstd, Albuera bajaría a ~8–9 GB (inferido, sin medir
la combinación).

## No medido

La geometría por separado (solo tramos de fichero completo), qué bytes de cada columna son de filas LOD (se supuso su
proporción de filas) y la combinación de candidatos.
