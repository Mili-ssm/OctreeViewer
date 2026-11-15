# RENDER-R1 T7: diagnóstico de la edad de escena

Fecha: 01-11-2026. Revisión: `ca78c30`. Estado: **explicado; sin corrección en R1 (lo resuelve R3)**.

Explica la subida de `sceneAgeMs` que dejó pendiente el [registro T7](RENDER_R1_T7_01-11-2026.md): en Belvedere 8 px, el p95 pasó de 120 ms a 2932 ms.

## Qué mide la métrica

`viewer-worker.js` guarda la hora de envío de cada vista (`viewSentAt`) y, cuando llega la **primera** escena construida para esa vista, registra el tiempo transcurrido. Las escenas siguientes con la misma vista no se cuentan: por eso hay 28–38 muestras para 67–114 escenas. La métrica es correcta y mide la latencia entre vista y escena.

## Causa: la etapa retenida de T1a

Antes de T1a, `orbit`, `pan`, `zoom` y `fit` ponían `lod_stage_pixels = None`: tras cada movimiento, la construcción volvía a la etapa más gruesa, que es barata. Desde `70fe384` (T1a) la etapa se conserva, así que cada vista nueva exige una construcción completa en la etapa fina, que dura segundos en Belvedere (`buildBusyByMessage.build` máx 5,4–7,5 s).

Evidencia: Belvedere 4 px, mismo arnés, 3 rondas por revisión ([datos](data/render-r1t7-01-11-2026/controls/)).

| Revisión | Muestras | `sceneAgeMs` p50 / p95 / máx | Escenas |
| --- | --- | --- | --- |
| A `ded7ee8` (antes de T1a) | 135–162 | 68–85 / 88–112 / 114–166 | 245–272 |
| B `70fe384` (T1a, antes de T3b) | 32 | 251–326 / 675–835 / 916–1665 | 114–119 |
| Actual `135c147` | 28–31 | 255–331 / 670–1150 / 1170–2039 | 108–113 |

El salto coincide con T1a; T3b no lo cambia. La ronda s3 de A no tiene `buildBusyByMessage`, pero sí `sceneAgeMs`.

## Lectura

- Es el coste aceptado de T1a: el corte visible ya no vuelve a una etapa más gruesa, a cambio de que la escena de una vista nueva tarde una construcción fina en llegar. A cambio también hay la mitad de escenas, y Belvedere 8 px llega antes al objetivo (66,8 → 48,8 s).
- Mientras tanto, el worker de render sigue dibujando la escena anterior a 60 Hz. Los cubos están en coordenadas de mundo, así que el giro es correcto; lo único desfasado son las regiones recién expuestas. En Majadas, los píxeles vacíos siguen en el rango de la línea base.
- No se corrige en R1. Reconstruir primero en grueso durante la órbita reabriría T1a, y no existe ningún margen de frustum que reduzca los bordes desfasados. R3 ([ADR 0019](../../adr/0019-cubos-por-nodo-residentes-en-gpu.md)) elimina la reconstrucción por vista, porque los cubos por nodo son independientes de la vista. Al medir R3, `sceneAgeMs` debe compararse con este registro.

## Límites

n = 3 por revisión, un solo equipo y solo Chrome. No se ha medido la fracción de píxeles vacíos en Belvedere antes de T1a.
