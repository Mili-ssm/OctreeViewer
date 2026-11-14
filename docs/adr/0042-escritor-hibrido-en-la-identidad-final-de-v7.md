# 0042 El escritor híbrido entra en la identidad final de V7, sin revisión `-pre` propia

Estado: Aceptada · Fecha: 14-11-2026 · Decide: propietario · Enmienda el [ADR 0005](0005-identidades-exactas-v7n-pre-por-candidato.md) para este caso

## Contexto
- El [ADR 0041](0041-orden-de-filas-libre-por-bloque.md) adopta un escritor que elige el orden de filas por bloque.
  El ADR 0005 da identidad propia (`v7.N-pre`) a cada heurística nueva del escritor.
- Una identidad `v7.6-pre` toca unos 30 puntos: versiones y magics, la E/S paginada fijada a `v7.5-pre`, el CLI,
  la especificación, los índices y las pruebas. El congelado de V7 ([ADR 0022](0022-alcance-y-congelacion-de-v7-final.md))
  crea la identidad final y repetiría ese trabajo.
- El congelado espera además a RQ-07 (plan C4), y los datasets se regeneran una sola vez en el congelado.

## Decisión
- El escritor híbrido no tiene revisión `-pre` propia. Se implementa y se prueba ya, detrás de una opción del
  escritor que por defecto conserva el orden de la fuente, y pasa a ser el escritor de la identidad final de V7
  en el congelado.
- Hasta el congelado, los ficheros escritos con la opción solo sirven para medir: no se publican datasets con
  ella, porque llevan la identidad `v7.5-pre` y una heurística distinta.

## Alternativas
| Opción | Por qué no |
| --- | --- |
| Identidad `v7.6-pre` ahora | Cumple el ADR 0005 al pie de la letra, pero hace dos veces el trabajo de identidad |
| Opción dentro de `v7.5-pre` sin límite de uso | Una identidad con dos heurísticas en ficheros publicados: comparaciones y reanudaciones ambiguas |

## Consecuencias
- Las comparaciones A/B entre los dos órdenes se hacen con la opción del escritor y se registran con ella.
- Un checkpoint de conversión no debe mezclar las dos opciones: la opción forma parte de la configuración de la
  conversión.
- El trabajo de identidad se hace una vez, en el congelado (plan D3c).

## Revisar si
El congelado se aplaza tanto que hacen falta datasets publicados con el escritor híbrido antes de él.
