# 0055 En modo celda la GPU decide el nivel dibujado y la CPU solo la residencia

Estado: Aceptada · Fecha: 18-11-2026 · Decide: propietario (propuso el agente; aceptada el 18-11-2026) · Precisa [ADR 0050](0050-nivel-por-celda-segun-distancia.md) y [ADR 0053](0053-transiciones-de-un-nivel.md); cambia la lectura de R3-Q21 y de RF-31

## Contexto
- La regla E4 de la GPU (`cell_draws` en `shaders/cell_compute.wgsl`, gemelo `cell_lod.rs::draws`) dibuja, entre los niveles
  activos de un nodo, el más grueso cuya celda no se refina. Parte cada punto en una sola celda (test de partición
  en GPU de la fase 2a: 0 huecos y 0 dobles).
- La CPU elige antes los niveles activos con una estimación por octante (sondas, octantes detrás del ojo, caja de
  datos, rango por nodo, pasos, ventana, semillas). Los huecos de las poses P4 y P2 de la fase 2b fueron conjuntos
  activos con saltos (`{11, 18}`), no fallos de E4
  ([diagnóstico](../records/archive/replanteamiento-lod-por-celda-18-11-2026.md#por-qué-se-atascó)).
- El propietario (18-11-2026) pide seguir la intención de los requisitos antes que su literalidad.

## Decisión
- **Activo = tramo contiguo.** Los niveles activos de un nodo son un tramo contiguo de niveles residentes: un
  nivel no se activa si entre él y lo que ya se dibuja falta un nivel residente. Lo que ya se dibuja no se suelta
  para cerrar un hueco (engrosaría la imagen).
- **La CPU decide la residencia.** Por nodo calcula un rango conservador `[lo, hi]` (lado lejano y lado cercano,
  este limitado a la distancia cercana del LOD) y con él pide, retiene y suelta niveles, dentro del presupuesto.
- **La GPU decide el dibujo** con E4, sin cambios.
- **Transiciones:** con el tramo sin huecos, dos celdas vecinas del mismo nodo difieren como mucho en un nivel;
  en el tiempo, los niveles entran conforme se hacen residentes. No hay ventana de un nivel por frame ni, al
  completar la propuesta, estado de pasos por octante.
- El modo octante queda congelado y no condiciona el modo celda; se retira tras validar la fase 2b en el
  navegador (propietario, 18-11-2026).
- La ventana de un nivel por frame se quita a prueba: si en la demo se ven saltos, vuelve (propietario, 18-11-2026).

## Alternativas
| Opción | Por qué no |
| --- | --- |
| Corregir solo el rango por nodo (`hi`) | Mantiene las sondas por octante y sus reglas; la siguiente pose expone el siguiente caso |
| Cadena entre nodos (F4 de la fase 2b) | Mayor; solo si los bordes de nodo siguen fallando con la puerta de residencia |
| Historia por celda para cumplir «un nivel por frame» literal | Estado por celda en GPU; el fin (suavidad) se alcanza sin él |

## Consecuencias
- Se pueden borrar del modo celda `wanted`/`drawn`/`stage`/`step`, las reglas de octantes detrás del ojo,
  `node_far_depth` y las semillas de entrada (estimación: 600–900 líneas más sus tests, sin medir).
- Los oráculos temporales pasan de (nodo, octante) a muestras de la rejilla; el gemelo y los tests de GPU no cambian.
- Con movimientos rápidos de cámara un punto puede cambiar más de un nivel entre dos frames: 280 saltos dentro del
  nodo en una órbita simulada de Belvedere, frente a 0 con la ventana
  ([experimento E-B1](../records/archive/replanteamiento-lod-por-celda-18-11-2026.md#experimentos)). Es el punto que el
  propietario debe aceptar o rechazar.
- Más niveles activos por nodo cerca del ojo: E4 evalúa cada antepasado activo; hay que medir el paso de celdas.

## Revisar si
El p95 del frame supera el doble del de la fase 2a, los bordes de nodo en pantalla difieren en más de un nivel, o
los bytes residentes superan el pool en Majadas dentro de una hoja.
