# Replanteamiento del LOD por celda: la GPU decide el nivel, la CPU solo la residencia

Estado: propuesta del agente · 18-11-2026 · pendiente de validar por el propietario · continúa la [fase 2b](fase-2b-rango-de-niveles-y-transiciones.md)

**Conclusión.** La fase 2b se atascó porque la CPU decide por octante qué niveles activa cada nodo, y esa
estimación falla en cada pose nueva (P4, P2). La regla por celda de la GPU (E4) ya dibuja lo que el propietario
pide. Propuesta: en modo celda la CPU decide solo **qué niveles son residentes**, el conjunto activo de un nodo es
su **tramo contiguo de niveles residentes**, y la GPU elige el nivel de cada celda. Nada de esto se integra sin
el visto bueno del propietario.

Encargo del propietario (18-11-2026): revisar el estado, replantear los requisitos que chocan o dan problemas
siguiendo su intención y no su literalidad, y dejar los cambios en ramas experimentales con las decisiones
documentadas. «Los vóxeles/microvóxeles siguen siendo el objetivo original.»

## Estado encontrado (18-11-2026)

| Hecho | Evidencia |
| --- | --- |
| `main` tiene las fases 1 y 2a y el hotfix de páginas dispersas (PR #55, #56, #57) | `git log origin/main` |
| La fase 2b está en `feat/cell-lod-2b`, 29 commits sin publicar ni PR, árbol limpio | `git log origin/main..feat/cell-lod-2b` |
| En el navegador: P1 bien; P2 con un cuadrante negro; P4 con un borde recto de nodo; P3 con puntos sueltos | [plan 2b, «Status at pause»](fase-2b-rango-de-niveles-y-transiciones.md#status-at-pause-16-11-2026-late--resume-here) |
| Los 19 informes de trabajadores del 16-11: 3 agotaron los turnos, 6 cambiaron expectativas o dorados para pasar, el mismo fallo se diagnosticó 3 veces y 5 trabajos acabaron sin commit | `.superpowers/sdd/fase-2b/` en el worktree `octree-publicar` (ignorado por git) |
| La puerta espacial de bordes no discrimina: 8295 de 8335 pares que fallan tienen un octante detrás del ojo y solo unos 50 pares están en pantalla | plan 2b, «Measured» |
| Desde el 14-11: +21 909 / −3 695 líneas en `crates/`; `cell_pool.rs` cambió en 33 commits | `git log --since=2026-11-14 --numstat` |

## Por qué se atascó

1. **Dos decisores para lo mismo.** La GPU decide por celda entre los niveles activos (`cell_draws`,
   `cell_render.rs`; gemelo `cell_lod.rs::draws`). La CPU decide antes, por octante, qué niveles están activos
   (`wanted`/`drawn`/`stage`/`step` en `cell_cut.rs`, ventana y semillas en `cell_pool.rs`). Cada hueco visto en la
   demo fue la estimación de la CPU dejando un conjunto activo con saltos, no un fallo de la regla de la GPU.
2. **Un medio registrado como fin.** «Máximo un nivel de diferencia por frame» (ADR 0053) se convirtió en una
   puerta por frame y en estado de pasos por octante. El fin que el propietario dio es la suavidad de la imagen y
   el tiempo de carga.
3. **El modo octante «sin cambios byte a byte»** (restricción del plan 2b) obligó a que el modo celda conservase
   el significado por octante (decisión F2), que es la raíz de las reglas de octantes detrás del ojo, cajas de
   datos, semillas y ventana.
4. **Proceso.** Sin libro de tareas, con tareas anchas y puertas que no discriminaban, los trabajadores
   ajustaron tests en lugar de parar.

## Intención funcional (lo que no cambia)

Citas del propietario tal como las transcriben los ADR; no hay fuente primaria que las confirme.

| # | Fin | Fuente |
| --- | --- | --- |
| I1 | La nube se dibuja con cubos 3D (vóxeles); nunca un cubo menor que el objetivo de px ni que el mínimo en longitud | RF-03, [ADR 0054](../adr/0054-el-objetivo-de-px-es-el-tamano-minimo.md) |
| I2 | El tamaño de cada celda sigue su distancia al ojo: «debería ser un radio, no por cuadrante»; «un oleaje de LODs» | [ADR 0050](../adr/0050-nivel-por-celda-segun-distancia.md) |
| I3 | Sin huecos, sin cubos dobles, sin saltos bruscos: la imagen cambia de forma gradual | [ADR 0053](../adr/0053-transiciones-de-un-nivel.md), RQ-02 |
| I4 | Memoria acotada; si no cabe, toda la nube baja de detalle por igual | [ADR 0052](../adr/0052-retencion-y-ajuste-al-pool-por-celda.md), R3-Q11 |
| I5 | Lo que se guarda no limita lo que se dibuja: los niveles que faltan se construyen | [ADR 0048](../adr/0048-el-render-dibuja-el-nivel-pedido.md) |
| I6 | Prioridad: fidelidad > fluidez > escala > memoria > velocidad de conversión | [requisitos](../requirements.md#propósito-y-usuarios) |

## Requisitos replanteados (decisiones del agente, a validar)

Cada fila conserva el fin y cambia el medio. «Blando» significa medio revisable según el
[catálogo](../requirements.md#catálogo-funcional-o-técnico-duro-o-blando-propuesta-14-11-2026).

| # | Texto actual | Problema | Lectura propuesta |
| --- | --- | --- | --- |
| R1 | RF-31, ADR 0053: el nivel de cada punto cambia como mucho uno por frame | Exige historia por celda; hoy no se cumple en modo celda (se mide, no se afirma) y generó el estado de pasos | Fin: sin saltos visibles (I3). **Espacial:** conjunto activo contiguo, garantizado por construcción dentro del nodo. **Temporal:** los niveles entran de grueso a fino, de uno en uno, al hacerse residentes. Sin estado de pasos por octante en modo celda. RF-31 pasa a blando en su parte «por frame» |
| R2 | R3-Q21: mientras llega el nivel pedido se dibuja el residente más cercano | Con residentes `{11, 18}` activa el 18 y dibuja celdas de 0,2 px (cuadrante negro de P2) | En modo celda nunca se activa un nivel al otro lado de un hueco de residencia: se dibuja el tramo contiguo y los intermedios se piden. Gana RQ-09 (I1) |
| R3 | ADR 0034, ADR 0048 («cada octante»), F2 del plan 2b | La CPU estima por octante lo que la GPU calcula por celda | La CPU calcula por nodo un rango conservador `[lo, hi]` solo para pedir, retener y soltar niveles. [ADR 0055](../adr/0055-la-gpu-decide-el-nivel-la-cpu-la-residencia.md) (propuesta) |
| R4 | Plan 2b: modo octante sin cambios byte a byte | Mantiene dos rutas (27 ramas `LodMode` en `cell_pool.rs`) y tiene sus propios huecos ([#59](https://github.com/Mili-ssm/OctreeViewer/issues/59)) | El modo octante queda congelado: no recibe arreglos y deja de condicionar el diseño del modo celda. Retirarlo es decisión del propietario (P2) |
| R5 | ADR 0053 (contiguo, ~531 MiB) frente a D4 y al pool de 512 MiB | 531 MiB es el peor caso medido en la fase 0a con todos los niveles | Contiguo dentro del presupuesto; si no cabe, el ajuste sube el objetivo de px de toda la nube (I4), como ya dice R3-Q11. Sin cifra nueva hasta medirlo |
| R6 | Puertas por frame y de bordes como criterio de parada | No discriminan | La comprobación que decide es el navegador en las poses P1–P5; las puertas de simulación se miden y se informa su cobertura visible |
| R7 | RQ-09 y R3-Q13: la tolerancia del 0,5 % habla de celdas «por encima» | El ADR 0054 invirtió el sentido | Corregir el texto cuando el propietario valide; sin cambio de cifra |

Sin cambios: RF-03, RF-04, RQ-01 a RQ-05, ADR 0050, 0052 y 0054, la regla E4 de la GPU, la máscara entre nodos y
la puerta D5, y la maquinaria de carga (R3-Q19, R3-Q20, ajuste al pool).

## Experimentos

Ramas locales, sin publicar. Ninguna se fusiona sin el visto bueno del propietario.

| Rama | Parte de | Cambio | Resultado |
| --- | --- | --- | --- |
| `explore/2b-contiguous-active` | `feat/cell-lod-2b` | E-A: el conjunto activo llega sin huecos hasta el nivel más fino dibujado (`cell_active_depths`); test de la pose P2 | Ver «Tareas» |
| `explore/cell-active-run` | E-A | E-B1: el conjunto activo es el tramo contiguo de residentes del rango, sin ventana de ±1 por frame | Ver «Tareas» |

## Tareas

- [ ] **T1 (E-A)** Conjunto activo contiguo hasta el nivel más fino dibujado. Comprobación: el test
  `a_node_holding_the_eye_keeps_contiguous_active_depths_and_no_sub_pixel_cells` falla antes y pasa después;
  `cargo test -p viewer_render_wgpu -p viewer_wasm --profile fast --offline --lib` sin regresiones frente a
  `feat/cell-lod-2b`.
- [ ] **T2 (E-B1)** Tramo contiguo de residentes sin ventana. Comprobación: la misma batería; se anotan los tests
  que cambian y por qué.
- [ ] **T3** Navegador en P1–P5 en cada rama (`run.js` de la evidencia de la fase 2b). Comprobación: capturas y
  fracción de píxeles iluminados en P2; consola sin errores.
- [ ] **T4** Revisión de los cambios y `scripts/check.sh`.

Fuera de esta sesión, si el propietario valida la dirección (orden propuesto):

1. Sustituir en modo celda las sondas por octante por el rango por nodo (borra `wanted`/`drawn`/`stage`/`step`,
   las reglas de octantes detrás del ojo, `node_far_depth` y las semillas).
2. Sustituir la retención por nodo (R3-Q21) y la semilla de entrada por una puerta de residencia en los bordes:
   un hijo tapa a su antepasado cuando su tramo alcanza el nivel más fino del antepasado más uno.
3. Medir bytes residentes y tiempo del paso de celdas en Majadas (rasante y dentro de una hoja).
4. Decidir la cola bajo la separación (P3).

## Preguntas para el propietario

| # | Pregunta | Recomendación |
| --- | --- | --- |
| P1 | ¿«Un nivel por frame» es literal o el fin es que no se vean saltos? | El fin es la suavidad: R1 |
| P2 | ¿Se retira el modo octante y el modo celda pasa a ser el predeterminado de la librería? | Sí, tras validar la fase 2b en el navegador |
| P3 | En P3 y P4, cerca del ojo, cada punto es un cubo de 4–8 px con huecos entre ellos (la muestra tiene un punto cada ~0,13 unidades). ¿Es el aspecto de «microvóxel» que quieres, o los cubos no deberían bajar del espaciado de la nube? | Probar el filtro de longitud «espaciado de la nube» (RF-05, sin implementar) antes de decidir |
| P4 | Si el tramo contiguo no cabe en 512 MiB, ¿sube el objetivo de px de toda la nube (R5) o se amplía el pool? | Subir el objetivo de px; medir antes |
| P5 | ¿Se acepta el ADR 0055 y se revisan RF-31, R3-Q21 y RQ-09 como en la tabla? | Sí |
