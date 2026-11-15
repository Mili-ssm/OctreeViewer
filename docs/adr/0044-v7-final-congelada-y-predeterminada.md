# 0044 V7 final se congela sobre v7.5-pre con el escritor híbrido y pasa a ser la salida predeterminada

Estado: Aceptada · Fecha: 15-11-2026 · Decide: propietario · Sustituye al [ADR 0002](0002-v5-predeterminado-en-transicion.md); ejecuta el congelado del [ADR 0022](0022-alcance-y-congelacion-de-v7-final.md) · Regla de hoja fija precisada por el [ADR 0051](0051-granularidad-de-hojas-configurable.md)

## Contexto
- R3, R4, R5 y la rejilla global están dentro de `v7.5-pre`. RQ-07 queda revisado por el
  [ADR 0043](0043-coste-de-conversion-de-v7-frente-a-v5.md). El [ADR 0042](0042-escritor-hibrido-en-la-identidad-final-de-v7.md)
  reserva el escritor híbrido para la identidad final.
- El formato paginado de `v7.5-pre` no admite conversiones con checkpoint o reanudación (`las_source.rs`) ni
  entrada EPT (`ept.rs`). RF-26 da por hecho que existen checkpoints; RF-20 solo pide entrada LAS/LAZ.
- Cambiar el codificador LZ4 ([ADR 0025](0025-codificador-lz4-propio.md)) no cambia el formato: cualquier
  decodificador LZ4 lee los bloques; solo cambian las SHA de referencia.

## Decisión
- **Identidad:** `FormatVersion::V7` (cabecera mayor 7, revisión 0; texto `v7`) con la disposición de bytes de
  `v7.5-pre` y magics propios de índice y pie (`OCTVIDX7`/`OCTVEND7`, como V5).
- **Escritor:** V7 escribe siempre con el orden de filas híbrido; el usuario no lo elige y pedir otro orden con V7
  es un error (ADR 0042: no hay dos heurísticas seleccionables en una identidad publicada). El orden de la fuente
  queda para las `-pre`. Un cambio futuro de la política del escritor sigue el [ADR 0041](0041-orden-de-filas-libre-por-bloque.md):
  no cambia la versión del formato, porque el lector admite cualquier orden por bloque.
- **Predeterminado:** `FormatVersion::DEFAULT_OUTPUT` pasa a `V7` en el CLI, el wasm y la app web, una vez medido
  RQ-07 con V7 (condición del ADR 0022; [plan](../plans/d3c-congelado-v7-final.md), Tarea 3). Las
  conversiones con checkpoint y la entrada EPT fallan con V7 y su error indica `--format-version v5`.
- **Sin cambio:** V5, V6 y las identidades `v6.N-pre` y `v7.N-pre` se siguen leyendo y escribiendo hasta la
  retirada ([ADR 0004](0004-retirar-v5-v6-tras-v7.md)). El codificador LZ4 propio va después del congelado.

## Alternativas
| Opción | Por qué no |
| --- | --- |
| Congelar sin cambiar el predeterminado | El propietario prefiere promover ya; el congelado es la condición del ADR 0002 |
| V7 por defecto con vuelta automática a V5 en checkpoint o EPT | El formato de salida cambiaría según la entrada sin que se pida |
| Revisión `v7.6-pre` antes de la final | Repite el trabajo de identidad (ADR 0042) |
| LZ4 propio antes del congelado | Más riesgo en el congelado para algo que no cambia el formato |

## Consecuencias
- Las SHA de referencia R1, los datasets de render y el corpus de fuzz se regeneran con V7.
- Las conversiones con checkpoint y la entrada EPT pasan a necesitar `--format-version v5`, hasta que el formato
  paginado admita checkpoints o se retire V5.
- La retirada de V5/V6 y de las `-pre` exige antes checkpoints en el formato paginado o una decisión sobre RF-26.
- El LZ4 propio, cuando llegue, cambia los bytes y las SHA de salida, pero los datasets V7 ya escritos siguen siendo
  válidos y no hace falta regenerarlos.

## Revisar si
- Un usuario necesita checkpoints o entrada EPT sin pasar a V5.
- Hace falta cambiar el formato V7 después del congelado: sería V8 o una extensión compatible.
