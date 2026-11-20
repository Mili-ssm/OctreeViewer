# 0059 El tamaño mínimo por defecto de la demo es el p5 del fichero cuando lo trae

Estado: Aceptada · Fecha: 20-11-2026 · Decide: propietario · Sustituye en parte al [ADR 0054](0054-el-objetivo-de-px-es-el-tamano-minimo.md) (el valor por defecto del mínimo en longitud); cierra la parte pendiente de D5 del [ADR 0058](0058-formato-v8-experimental.md)

## Contexto
- El ADR 0054 fija el mínimo en longitud por defecto en 0,1 mm para cualquier nube; la resolución de OQ-13
  (18-11-2026) lo mantiene como aspecto de cerca por defecto.
- Desde `v8.1-pre`, el fichero guarda los cuantiles p1/p5/p50 de la distancia al vecino más cercano
  ([FORMAT_V8_PRE1](../reference/format/FORMAT_V8_PRE1.md)); p5 es el tamaño mínimo de vóxel recomendado (D5) y el
  visor ya lo expone como `cellsInfo().recommendedMinSize`. Todo ceros significa «sin medida».
- Medido el 20-11-2026 ([registro](../records/benchmarks/FORMATO_V8_PRE1_20-11-2026.md)): p5 vale ~0,17 unidades en
  Belvedere y ~0,001 en Majadas; un valor fijo de 0,1 mm no se ajusta a nubes de densidades tan distintas.
- Supuesto: p5 de una muestra de ~2¹⁶ puntos representa la separación mínima útil de la nube (un percentil bajo
  resiste duplicados y ruido mejor que el mínimo).

## Decisión
El propietario (20-11-2026), tras medir v8.1-pre: **la demo usa como mínimo en longitud por defecto el p5 del
fichero cuando lo trae (p5 > 0)**; sin p5 (v7 o «sin medida») sigue en 0,1 mm. El usuario lo puede cambiar como hoy;
el resto del ADR 0054 (el objetivo de px es un mínimo, gana el filtro mayor) no cambia.

## Alternativas
| Opción | Por qué no |
| --- | --- |
| Mostrar p5 como sugerencia sin aplicarlo | El valor por defecto seguiría sin ajustarse a la densidad de cada nube |
| Esperar a congelar la V8 | El dato ya está en los ficheros v8.1-pre y el cambio en la demo es pequeño y reversible |

## Consecuencias
- Cambian la demo (`examples/web-host`) y su README, RQ-09 y la fila de OQ-13 de los requisitos; se verifica en el
  navegador con un fichero v8.1-pre y con uno v7.
- El aspecto de cerca por defecto depende del fichero: dos nubes con la misma unidad pueden abrir con mínimos
  distintos.

## Revisar si
p5 recorta detalle útil o deja la cola de celdas bajo la separación demasiado grande en alguna nube medida, o cambia
cómo se calculan los cuantiles.
