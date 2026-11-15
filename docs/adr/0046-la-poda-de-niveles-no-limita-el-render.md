# 0046 La poda de la cadena de niveles es de almacenamiento; el render deriva el nivel que pide la vista

Estado: Aceptada · Fecha: 15-11-2026 · Decide: propietario · Precisa los ADR [0031](0031-celdas-por-nodo-en-v7-5.md) y [0032](0032-hojas-adaptativas-e-instancia-de-12-b.md)

## Contexto
- Cada nodo guarda una cadena de niveles de celdas. Las hojas bajan hasta separar todas sus posiciones
  (ADR 0032), y la cadena se poda con `chain_keep_mask` (crecimiento mínimo 1,5; ADR 0031).
- En una nube con puntos a ~1 cm, sus hojas solo separan sus posiciones 4–6 niveles por debajo de su
  espaciado, con casi el mismo número de celdas. La poda descarta entonces todos los niveles gruesos, y el
  render dibuja la hoja con celdas de 0,006–0,03 px: la nube se ve como un punto
  ([#35](https://github.com/Mili-ssm/OctreeViewer/issues/35)).
- El propietario (15-11-2026): «una cosa es como comprimimos y otra como renderizamos». Si la vista dibuja
  vóxeles de 50 m, una hoja que solo guarda niveles finos debe verse con vóxeles de 50 m, igual que sus vecinas.

## Decisión
- La poda decide solo qué niveles se **guardan**. No limita qué nivel se **dibuja**.
- Cuando la vista pide para un nodo un nivel más grueso que todos los guardados, el render lo deriva del nivel
  guardado más cercano agrupando sus celdas en celdas del tamaño pedido. Las celdas finas no se dibujan tal
  cual ni se inflan.
- El resto de reglas de R3 sigue igual: la imagen solo se refina (R3-Q21) y RQ-09 sigue fijando el tamaño en
  pantalla.

## Alternativas
| Opción | Por qué no |
| --- | --- |
| Las hojas guardan siempre su nivel de espaciado | Mezcla almacenamiento y render: más memoria (+96 % de celdas en el caso de #35) para resolver un problema de dibujo |
| Poda acotada por niveles | Igual que la anterior: cambia lo que se guarda |
| Inflar las celdas finas | Cubos solapados; no es el nivel pedido y rompe RQ-09 |
| Continuidad activada por defecto | Oculta el defecto sin resolverlo |

## Consecuencias
- El corte, la residencia y el worker de construcción tienen que poder pedir y construir un nivel derivado.
  Agrupar es barato: desplazar las claves de celda y eliminar duplicados.
- La implementación va en #35 con un plan propio. El test `leaf_with_a_close_pair_keeps_a_level_at_its_spacing`
  del informe de #35 se reescribe para probar el nivel dibujado, no el guardado.

## Revisar si
Derivar niveles en el render cuesta más tiempo de frame que guardarlos, medido en Majadas y Belvedere.
