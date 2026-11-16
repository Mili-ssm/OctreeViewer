# 0049 El tamaño mínimo del cubo se elige en px, en longitud o por el espaciado de la nube

Estado: Aceptada (revisada el 16-11-2026: filtros combinables, ver «Revisión») · Fecha: 15-11-2026 · Decide: propietario · Precisa RF-05, RQ-09 y el [ADR 0019](0019-cubos-por-nodo-residentes-en-gpu.md)

## Contexto
- Hoy el detalle se controla con el objetivo de px. La continuidad (RF-05) es una casilla que fija el lado mínimo del cubo
  en el espaciado de referencia de la nube.
- El propietario (15-11-2026) quiere seguir refinando por debajo del espaciado ([ADR 0048](0048-el-render-dibuja-el-nivel-pedido.md))
  y poder elegir el tamaño mínimo del cubo («vóxel», en sus palabras) en px o en m/cm/mm.
- El visor guarda el CRS sin interpretarlo; no sabe en qué unidad lineal está una nube.

## Decisión
- Un selector **«Tamaño mínimo»** con tres modos:
  - **Solo px** (predeterminado): sin mínimo en el mundo; los cubos bajan hasta el objetivo de px, también por
    debajo del espaciado.
  - **Longitud**: valor y unidad (m, cm, mm). Ningún cubo tiene un lado menor que esa longitud.
  - **Espaciado de la nube**: la continuidad de RF-05, que pasa a ser este modo.
- El selector sustituye al modo de huecos del ADR 0019: «solo px» es el modo literal y «espaciado de la nube» la
  continuidad. En ningún modo se dibujan cubos donde la fuente no tiene puntos (RF-04).
- En los modos «longitud» y «espaciado», un cubo puede quedar por encima del objetivo de px: RQ-09 lo admite
  como elección del usuario.
- El objetivo de px se mantiene en todos los modos: acota el detalle a lo lejos.
- **Unidades:** se lee la unidad lineal del WKT del CRS (metro, pie…) y se convierte. Si el CRS no la declara
  (solo un código EPSG, o sin CRS), se asumen metros y el control lo indica. Con un CRS geográfico (grados) el
  modo «longitud» se desactiva con un aviso.

## Alternativas
| Opción | Por qué no |
| --- | --- |
| Asumir siempre metros | Tamaños equivocados en nubes en pies o en grados |
| Unidades del CRS sin m/cm/mm | Correcto, pero menos claro para quien mira la demo |
| Dos modos y la casilla de continuidad aparte | Dos controles que se solapan |

## Consecuencias
- La casilla de continuidad desaparece de la demo; `setContinuity` sigue en la API para el modo «espaciado».
  Cambian `viewer-worker.js`, `tests/cells-protocol.test.mjs` y `probe-cells-continuity.mjs`.
- El modo «longitud» necesita leer la unidad del CRS al abrir la nube.

## Revisar si
Aparece una nube con CRS geográfico (grados) que necesite tamaño mínimo en longitud.

## Revisión 16-11-2026: dos filtros combinables, gana el más grande
- El propietario (16-11-2026): «poder elegir tanto mínimo en px como mínimo en m y que en cada frame/vóxel gane el
  filtro más grande». El selector de tres modos exclusivos se sustituye por **dos filtros que actúan a la vez**:
  - **Mínimo en px**: el objetivo de px de siempre; siempre activo.
  - **Mínimo en longitud**: desactivado (predeterminado), un valor con unidad (m, cm, mm) o el espaciado de la nube
    (la continuidad de RF-05).
- En cada celda y cada frame el lado dibujado no baja de `max(lado_mín, lado cuya arista proyectada cumple el
  objetivo de px)`: es la regla D3 del [plan](../plans/nivel-pedido-y-lod-por-celda.md), que ya toma el máximo. «Solo px»
  es el filtro de longitud desactivado.
- Sigue todo lo demás: unidades del CRS, modo longitud desactivado con un CRS geográfico, RQ-09 admite cubos por
  encima del objetivo cuando gana el filtro de longitud, y `setContinuity` queda para el espaciado.
