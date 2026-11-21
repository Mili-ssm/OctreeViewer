# 0047 V5 queda como formato heredado de comparación hasta que empiece V8; la demo usa la última versión estable

Estado: Cumplida por el [ADR 0058](0058-formato-v8-experimental.md) y el [plan de congelado de la V8](../records/archive/formato-v8-congelado.md) (21-11-2026) · Fecha: 15-11-2026 · Decide: propietario · Precisa el [ADR 0004](0004-retirar-v5-v6-tras-v7.md)

## Contexto
- El ADR 0004 retira V5/V6 «al completar V7». V7 final ya es la salida predeterminada
  ([ADR 0044](0044-v7-final-congelada-y-predeterminada.md)), pero la retirada exige antes checkpoints en el formato
  paginado o una decisión sobre RF-26.
- La muestra integrada de la demo era V5. Se dibujaba por la ruta CPU y no enseñaba la ruta de celdas de V7.
- El propietario (15-11-2026): «la nube demo debería utilizar la última versión estabilizada; el v5 actualmente
  debería ser un legacy comparativo, que deberíamos eliminar cuando se empiece, si se empieza, un v8».

## Decisión
- **V5 es un formato heredado de comparación.** Se sigue leyendo y escribiendo para medir V7 frente a V5
  (RQ-07) y para checkpoints y entrada EPT, que V7 no admite. No recibe mejoras.
- **Se retira al empezar V8**, si llega a empezar; hasta entonces no se retira.
- **La demo y sus muestras usan la última versión estable del formato** (hoy V7). Los ficheros V5 que quedan son
  referencias heredadas con el sufijo `-v5`.
- V6 y las identidades `-pre` siguen como dicen los ADR 0004 y 0044.

## Alternativas
| Opción | Por qué no |
| --- | --- |
| Retirar V5 ya (ADR 0004 al pie de la letra) | Sin V5 no hay checkpoints ni entrada EPT, y se pierde la referencia de RQ-07 |
| Mantener V5 sin fecha de retirada | Coste de mantenimiento sin final (motivo del ADR 0004) |

## Consecuencias
- `examples/web-host/sample.oct` pasa a V7, generado por el conversor. La muestra V5 se conserva como
  `sample-v5.oct` para las sondas heredadas.
- Antes de empezar V8, el formato paginado tiene que admitir checkpoints y EPT, o hay que decidir sobre RF-26.
  Si no, retirar V5 deja esas entradas sin formato.

## Revisar si
Empieza V8, o una conversión real necesita una mejora de V5 que V7 no pueda dar.
