# 0043 El coste de conversión de V7 final se acota a ≤ 2,1× el tiempo y ≤ +40 % el RSS de V5

Estado: Aceptada · Fecha: 15-11-2026 · Decide: propietario · Revisa la cifra de conversión de RQ-07 ([requisitos](../requirements.md#requisitos-de-calidad))

## Contexto
- RQ-07 pedía que convertir a V7 final costase ≤ +30 % de tiempo y de RSS frente a V5. El
  [catálogo](../requirements.md#catálogo-funcional-o-técnico-duro-o-blando-propuesta-14-11-2026) lo clasifica
  como técnico y blando: el 30 % era una elección, revisable si una ganancia de lectura o disco compensa más.
- Medida en Belvedere, en este equipo y con el perfil de medida `perf` ([registro C4](../records/benchmarks/C4_DEFERRED_COPY_15-11-2026.md#razón-v75v5-actual)):
  v7.5 con orden híbrido tarda ×1,99 (×2,03 con el orden de la fuente) y usa +36 % de RSS pico (+39 %), pero
  ocupa un 10,7 % menos en disco que V5.
- El tiempo se va en construir la jerarquía, sobre todo en accesos aleatorios a memoria. TC1, TC2(b) y TC3 dan
  ≤ 8 % ([registro](../records/benchmarks/EXPLORE_CONVERT_SPEED_14-11-2026.md)) y la copia diferida (C4) −1,8 %
  en la jerarquía. Volver a +30 % exige rediseñar el pipeline (TC5), sin estimación de coste.
- Supuesto: Belvedere representa la razón en otros datasets. Solo se midió ahí.

## Decisión
- La parte de conversión de RQ-07 pasa a ser: **tiempo ≤ 2,1× y RSS pico ≤ +40 % frente a V5**, en el mismo
  dataset, equipo y perfil (`perf`, el de medida). El resto de RQ-07 (lectura y navegación iguales o mejores; disco) no cambia.
- El congelado de V7 (D3c) sigue sin esperar a TC5. TC5 no se planifica ahora.

## Alternativas
| Opción | Por qué no |
| --- | --- |
| TC5 antes de congelar | Retrasa V7 final y la API pública por un coste de conversión que el usuario paga una vez por dataset; sin estimación |
| Revisar ahora y planificar TC5 después | Compromete trabajo sin un requisito que lo pida; se reabre con «Revisar si» |
| Mantener +30 % | Bloquea la promoción de V7 sin plan viable para cumplirlo |

## Consecuencias
- La conversión de V7 cuesta el doble que la de V5; el disco y la lectura compensan.
- El margen sobre la medida actual es pequeño (×1,99 frente a ×2,1; +36 % frente a +40 %): una regresión del
  conversor de unos pocos puntos incumple RQ-07. Se comprueba al promover V7, con rondas intercaladas.

## Revisar si
- Una medida en otro dataset de referencia supera ×2,1 o +40 %.
- La conversión pasa a ser un coste recurrente para el usuario, como reconvertir con frecuencia o convertir
  en la web datasets de escala ESC-1.
