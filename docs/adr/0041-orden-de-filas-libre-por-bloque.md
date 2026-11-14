# 0041 Orden de filas libre por bloque en V7 y escritor híbrido

Estado: Aceptada · Fecha: 14-11-2026 · Decide: propietario (OQ-09) · Relacionado: [ADR 0022](0022-alcance-y-congelacion-de-v7-final.md) (alcance de V7) y [ADR 0005](0005-identidades-exactas-v7n-pre-por-candidato.md)

## Contexto
- El contrato actual conserva el orden de filas de la fuente dentro de cada bloque. Es un requisito técnico blando
  ([catálogo](../requirements.md#catálogo-funcional-o-técnico-duro-o-blando-propuesta-14-11-2026)): se revisa si el
  ahorro medido supera el 5 %.
- En Belvedere v7.5, escribir cada bloque en orden Morton cuando eso reduce su geometría deja el `.oct` un 35 % más
  pequeño y no descarga más bytes por vista; la carga remota tarda lo mismo a 4 px y un ~6 % más a 1 px,
  dentro de la variación entre rondas ([registro](../records/benchmarks/OQ09_MORTON_ORDER_14-11-2026.md)). Ordenar todos los bloques carga antes, pero
  descarga un 34–49 % más por vista.
- El decodificador ya acepta los dos órdenes por bloque: los bloques de diferencias no dependen del orden y los de
  ocupación llevan una bandera por bloque.

## Decisión
- **V7 no garantiza el orden de la fuente.** Dentro de cada bloque, el orden de las filas lo elige el escritor; es
  el mismo para posiciones, colores y todos los atributos, y el índice de fila de las lecturas se refiere a ese orden.
- **Escritor por defecto híbrido:** cada bloque se escribe en orden Morton si su geometría codificada ocupa menos que
  en el orden de la fuente; si no, en el de la fuente (empate: fuente).
- Tras el congelado, la política del escritor puede cambiar (otra regla, o todo Morton) sin nueva versión del
  formato, porque el lector ya admite cualquier orden por bloque. Mientras V7 sea `-pre`, una heurística nueva
  sigue llevando identidad propia (ADR 0005).

## Alternativas
| Opción | Por qué no |
| --- | --- |
| Todo Morton | Carga remota un 16–24 % antes a 100 Mbit/s, pero un 34–49 % más de bytes por vista; peor en redes lentas |
| Conservar el orden de la fuente | Renuncia a un 35 % del `.oct` sin ganancia medida a cambio |

## Consecuencias
- El contrato de `OctreeReader` (mismo orden en todas las lecturas de un bloque) se mantiene; cambian los textos que
  prometen el orden de la fuente (`occupied_point_block.rs:33,77` en `viewer_core`,
  [FORMAT_V7_PRE2](../reference/format/FORMAT_V7_PRE2.md)) y las pruebas de paridad que comparan filas en orden de
  la fuente: deben comparar el conjunto de filas o reordenar antes.
- Una herramienta que necesite el orden de la fuente tiene que guardarlo aparte; ninguna lo necesita hoy.
- La etapa de bloques de la conversión tarda más (~2× en una ejecución del experimento). El tiempo total y el RSS
  de la conversión híbrida, que RQ-07 también acota, no se midieron: la implementación debe medirlos y buscar una
  regla de elección más barata que codificar cada bloque dos veces.
- Los datasets de referencia se regeneran en el congelado (ADR 0022), así que no hay migración aparte.

## Revisar si
- La investigación de por qué los ficheros todo Morton construyen las celdas un 35–50 % más rápido muestra que el
  orden de los bloques gruesos es la causa y compensa sus bytes.
- El coste de conversión del escritor híbrido no cabe en RQ-07.
