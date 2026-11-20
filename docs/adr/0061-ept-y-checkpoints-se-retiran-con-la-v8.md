# 0061 La entrada EPT y los checkpoints se retiran al congelar la V8 y se aplazan

Estado: Aceptada · Fecha: 21-11-2026 · Decide: propietario · Precisa el [ADR 0047](0047-v5-heredado-hasta-v8.md) (condición de retirada de V5) y acota RF-26

## Contexto
- D4 ([ADR 0058](0058-formato-v8-experimental.md), [diseño](../plans/formato-v8.md)) deja **solo V8**: se retiran v5,
  v6, v7 y todas las revisiones previas, y el visor solo lee V8.
- Hoy la entrada EPT y `--checkpoint`/`--resume` solo funcionan con `v5` y `v7.1-pre`–`v7.3-pre`. Los formatos de marco
  global (v7, v8) rechazan EPT, y los paginados rechazan los checkpoints
  ([inventario](../reference/inventory.md#entrada-checkpoints-y-formato)).
- El ADR 0047 pedía, antes de retirar V5, checkpoints y EPT en el formato paginado o una decisión sobre RF-26. RF-26 es
  un requisito técnico blando ([requisitos](../requirements.md)), y ningún requisito funcional exige la entrada EPT
  (RF-20: LAS/LAZ).
- Mantener v5 solo para EPT y checkpoints no sirve: tras D4 el visor no leería esa salida.
- La ruta por defecto (v7) tampoco tiene EPT ni checkpoints, así que su usuario no pierde nada.

## Decisión
El propietario (21-11-2026): **al congelar la V8 se retiran la entrada EPT y los checkpoints (`--checkpoint`,
`--resume`, `--verify-source`)**, con su código y sus pruebas. Se implementarán más adelante sobre V8, si se retoman
([#12](https://github.com/Mili-ssm/OctreeViewer/issues/12) pasa a V8). Hasta entonces, RF-26 no aplica.

## Alternativas
| Opción | Por qué no |
| --- | --- |
| Implementar checkpoints (y EPT) sobre V8 antes de congelar | Un subproyecto propio que retrasa D4 |
| Solo checkpoints antes de congelar | Mismo retraso, menor; el propietario prefiere congelar ya |
| Conservar v5 para EPT y checkpoints | El visor solo V8 no leería su salida |

## Consecuencias
- El conversor solo acepta LAS/LAZ/COPC. Pedir EPT, `--checkpoint` o `--resume` da un error claro.
- Una conversión larga (ESC-1, ~400 GB de LAZ, del orden de un día) que falle no se puede reanudar: se repite entera.
- La fila de RF-26 y el inventario cambian. #12 se reescribe para V8, y #28/#13 se cierran con D4.

## Revisar si
Se planifica ESC-1 en serio o una conversión larga falla en la práctica (entonces checkpoints sobre V8, #12), o aparece
un dataset que solo exista como EPT.
