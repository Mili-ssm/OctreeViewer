# Formato nativo `.oct`, v8.1-pre (esqueleto)

Identidad de desarrollo: cabecera mayor **8**, revisión **1**, texto `v8.1-pre`; opt-in. La salida predeterminada
sigue siendo [v7](FORMAT_V7.md). Decisiones en el [ADR 0058](../../adr/0058-formato-v8-experimental.md); diseño en
[formato-v8](../../plans/formato-v8.md) y plan en [formato-v8-pre1](../../plans/formato-v8-pre1.md).

> Estado: solo identidad. Hoy los bytes de `v8.1-pre` son los de [v7](FORMAT_V7.md) salvo la cabecera y los magics.
> La tabla de diferencias se completa en la tarea 8 del plan, cuando los cambios de carga útil estén escritos.

## Identidad y diferencias con v7

| Campo | v7 | v8.1-pre |
| --- | --- | --- |
| Revisión de cabecera (`.oct` y `.octattrs`) | `0` | `1` (mayor `8`) |
| Magic del índice | `OCTVIDX7` | `OCTVI81P` |
| Magic del pie (ambos ficheros) | `OCTVEND7` | `OCTVE81P` |
| Capacidades (páginas, marco global, códecs, planos de bytes, filas híbridas fijas) | las de v7 | las mismas |
| Checkpoints por bloques | no | no |
| Monolítico | no: pareja `.oct` + `.octattrs` | no: pareja `.oct` + `.octattrs` |

## Cambios previstos de carga útil (aún no escritos)

| Área | Cambio | Decisión |
| --- | --- | --- |
| Color de dibujo | se deriva de los datos guardados en lugar de guardarse dos veces (`derives_display_attribute`) | D2 |
| Puntos LOD | los nodos interiores omiten los atributos que el visor no dibuja (`omits_lod_attributes`) | D2 |
| Distancia mínima | cuantiles de la distancia al vecino más cercano por nodo (`stores_distance_quantiles`) | D5 |
