# R3 B3: continuidad por nube en la ruta de celdas (Tarea 13) — 14-11-2026

**Estado: pasa, con efecto visual mínimo en estos datos.** La continuidad de RF-05 ya se puede activar
por nube: ninguna celda se dibuja con lado menor que un mínimo de factor × espaciado de referencia de la
nube (factor 1 por defecto). Activarla o desactivarla no reconstruye celdas, y el espaciado de
referencia no cambia al injertar páginas. En Belvedere y Majadas el mínimo apenas actúa: solo cambia la
imagen de Majadas dentro de una hoja, y a simple vista no se distingue del modo literal.

| Criterio de la Tarea 13 ([plan](../archive/render-r3-cubos-en-gpu.md#tarea-13--continuidad-entre-nubes-rf-05-con-paginación)) | Resultado |
| --- | --- |
| Usar el espaciado de referencia de T0b | **Pasa:** mínimo = factor × `reference_spacing` de la nube (metadatos v7.5) |
| Un mismo espaciado por nube no cambia al injertar páginas | **Pasa:** el test con los dos workers compara el espaciado antes y después de injertar páginas |

## Cómo funciona

- `CellPool::set_min_side` limita la profundidad deseada de cada octante al nivel guardado más fino
  cuyo lado de celda no baja del mínimo; si todos son más finos, usa el más grueso guardado. El límite
  entra en la clave de vista, así que el frame siguiente replanifica sin pedir celdas nuevas.
- Lo que el límite engrosa **no** cuenta como etapa ni como `poolCoarsened`: es el aspecto pedido, no un
  objetivo sin alcanzar (R3-Q3 no cambia).
- API `setContinuity(enabled, factor?)`, casilla en la demo y estadísticas `continuity`,
  `continuityFactor` y `continuityMinSide`. Una nube sin espaciado de referencia sigue en modo literal.
  Una sola nube por visor: la multinube (RF-01) queda fuera.
- El modo literal sigue siendo el predeterminado (P4).

## Navegador

Probe `probe-cells-continuity.mjs`, Chrome con WebGPU, una ejecución por caso.

| Dataset · pose · px | Lado mínimo | Nodos construidos al alternar | Asentado (literal / continuidad) | Cambia la imagen |
| --- | --- | --- | --- | --- |
| Belvedere · encuadre · 4 y 1 px | 0,25 | 0 | sí / sí | no |
| Majadas · encuadre · 4 y 1 px | 0,00195 | 0 | sí / sí | no |
| Majadas · hoja +y · 4 y 1 px | 0,00195 | 0 | sí / sí | sí (digest), no a simple vista |

`poolStages` y `poolCoarsened` valen 0 en todas las ejecuciones; al desactivar la continuidad vuelve el
digest del modo literal. En la consola, solo el 404 sin URL de siempre.

## Límites

- **Efecto mínimo con factor 1:** el espaciado de referencia es la mediana ponderada de las hojas. En
  Majadas (TLS) vale ~2 mm, dominado por la zona densa junto al escáner. Los huecos visibles están en
  zonas poco densas, y una regla global (RF-05: «no por región») con factor 1 no los cubre. Si se busca
  un efecto visible, hay que subir el factor; es decisión del propietario.
- La calidad visual de la continuidad de cerca no se juzgó más allá de comparar capturas.
- Las capturas quedan fuera del repositorio (`target/r3-b3/`).

## Identidad y reproducción

- Código: rama `explore/b3-continuity`, revisiones `4028ff9` y `c5ce8ff`, más `1d20cef` de la revisión
  (unidades con `visual_scale` ≠ 1, contabilidad con el pool ajustado y factor validado), integradas
  después en `feat/r3-v75`. Entorno: el de [T11](data/r3-t11-smoke-13-11-2026/identity.txt).
- Datos en [data/r3-b3-continuity-14-11-2026](data/r3-b3-continuity-14-11-2026/).
