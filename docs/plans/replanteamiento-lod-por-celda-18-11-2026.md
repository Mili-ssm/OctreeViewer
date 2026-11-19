# Replanteamiento del LOD por celda: la GPU decide el nivel, la CPU solo la residencia

Estado: aprobado · 19-11-2026 · ejecutado en el [plan del ADR 0055](../records/archive/adr-0055-rango-por-nodo.md) (salida reducida, [registro de cierre](../records/benchmarks/R3_CIERRE_RANGO_POR_NODO_19-11-2026.md)); dirección validada por el propietario ([respuestas](../records/archive/retomar-18-11-2026.md#respuestas-del-propietario-18-11-2026)); continúa la [fase 2b](fase-2b-rango-de-niveles-y-transiciones.md)

**Conclusión.** La fase 2b se atascó porque la CPU decide por octante qué niveles activa cada nodo, y esa
estimación falla en cada pose nueva (P4, P2). La regla por celda de la GPU (E4) ya dibuja lo que el propietario
pide. Propuesta: en modo celda la CPU decide solo **qué niveles son residentes**, el conjunto activo de un nodo es
un **tramo contiguo de niveles residentes**, y la GPU elige el nivel de cada celda. Dos experimentos en ramas
locales dan el primer paso: el cuadrante negro de la pose P2 desaparece. Nada se integra sin el visto bueno del
propietario.

Glosario: **E4** es la regla de dibujo por celda de la GPU y **D5** la puerta que cubre la zona libre de una celda
gruesa con máscara ([plan de la fase 2a](../records/archive/fase-2a-lod-por-celda-en-gpu.md)); **F2** y **F4** son decisiones del
[plan de la fase 2b](fase-2b-rango-de-niveles-y-transiciones.md); **P1–P5** son las poses de cámara de la muestra
integrada que usa ese plan; **Q1–Q5** son las preguntas de este documento.

Encargo del propietario (18-11-2026): revisar el estado, replantear los requisitos que chocan o dan problemas
siguiendo su intención y no su literalidad, y dejar los cambios en ramas experimentales con las decisiones
documentadas. «Los vóxeles/microvóxeles siguen siendo el objetivo original» (mensaje del encargo, no recogido en ningún ADR).

## Estado encontrado (18-11-2026)

| Hecho | Evidencia |
| --- | --- |
| `main` tiene las fases 1 y 2a y el hotfix de páginas dispersas (PR #55, #56, #57) | `git log origin/main` |
| La fase 2b está en `feat/cell-lod-2b`, 29 commits sin publicar ni PR, árbol limpio | `git log origin/main..feat/cell-lod-2b` |
| En el navegador: P1 bien; P2 con un cuadrante negro; P4 con un borde recto de nodo; P3 con puntos sueltos | [plan 2b, «Status at pause»](fase-2b-rango-de-niveles-y-transiciones.md#status-at-pause-16-11-2026-late--resume-here) |
| Los informes y diagnósticos de trabajadores del 16-11 (19 documentos): 3 agotaron los turnos, 6 cambiaron expectativas o dorados para pasar, el mismo fallo se diagnosticó 3 veces y 5 trabajos acabaron sin commit | `.superpowers/sdd/fase-2b/` en el worktree `octree-publicar` (ignorado por git) |
| La puerta espacial de bordes no discrimina: 8295 de 8335 pares que fallan tienen un octante detrás del ojo y solo unos 50 pares están en pantalla | plan 2b, «Measured» |
| Desde el 14-11: unas +22 000 / −3 500 líneas en `crates/`; `cell_pool.rs` cambió en más de 30 commits | `git log --since=2026-11-14 --numstat -- crates/` en `feat/cell-lod-2b` |

## Por qué se atascó

1. **Dos decisores para lo mismo.** La GPU decide por celda entre los niveles activos (`cell_draws`,
   `shaders/cell_compute.wgsl`; gemelo `cell_lod.rs::draws`). La CPU decide antes, por octante, qué niveles están activos
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

Citas del propietario tal como las transcriben los ADR; no hay fuente primaria que las confirme. «Sin saltos» en I3 es la lectura del agente del fin que da el ADR 0053 (suavidad de la imagen).

| # | Fin | Fuente |
| --- | --- | --- |
| I1 | La nube se dibuja con cubos 3D (vóxeles); nunca un cubo menor que el objetivo de px ni que el mínimo en longitud | RF-03 (cubos), RQ-09 y [ADR 0054](../adr/0054-el-objetivo-de-px-es-el-tamano-minimo.md) |
| I2 | El tamaño de cada celda sigue su distancia al ojo: «debería ser un radio, no por cuadrante»; «un oleaje de LODs» | [ADR 0050](../adr/0050-nivel-por-celda-segun-distancia.md) |
| I3 | Sin huecos, sin cubos dobles, sin saltos bruscos: la imagen cambia de forma gradual | [ADR 0053](../adr/0053-transiciones-de-un-nivel.md); RQ-02 para los huecos |
| I4 | Memoria acotada; si no cabe, toda la nube baja de detalle por igual | [ADR 0052](../adr/0052-retencion-y-ajuste-al-pool-por-celda.md), R3-Q11 |
| I5 | Lo que se guarda no limita lo que se dibuja: los niveles que faltan se construyen | [ADR 0048](../adr/0048-el-render-dibuja-el-nivel-pedido.md) |
| I6 | Prioridad: fidelidad > fluidez > escala > memoria > velocidad de conversión | [requisitos](../requirements.md#propósito-y-usuarios) |

## Requisitos replanteados (decisiones del agente, a validar)

Cada fila conserva el fin y cambia el medio. «Blando» significa medio revisable según el
[catálogo](../requirements.md#catálogo-funcional-o-técnico-duro-o-blando-aceptado-19-11-2026).

| # | Texto actual | Problema | Lectura propuesta |
| --- | --- | --- | --- |
| R1 | RF-31, ADR 0053: el nivel de cada punto cambia como mucho uno por frame | Exige historia por celda; hoy no se cumple en modo celda (se mide, no se afirma) y generó el estado de pasos | Fin: sin saltos visibles (I3). **Espacial:** conjunto activo sin huecos dentro del nodo (hoy se cumple alrededor de cada nivel dibujado; entre dos niveles dibujados puede quedar un hueco hasta que los intermedios son residentes). **Temporal:** los niveles entran al hacerse residentes, pedidos primero los adyacentes; con movimientos rápidos un punto puede cambiar más de un nivel entre dos frames (medido en E-B1). RF-31 pasaría a blando en su parte «por frame» si el propietario lo acepta (Q1) |
| R2 | R3-Q21: mientras llega el nivel pedido se dibuja el residente más cercano | Con residentes `{11, 18}` activa el 18 y dibuja celdas de 0,2 px (cuadrante negro de P2) | En modo celda un nivel del rango no se activa si entre él y lo que ya se dibuja falta un nivel residente; los intermedios se piden. Gana RQ-09 (I1). Lo que ya se dibuja no se suelta: anclar el tramo en el nivel más grueso engrosaba la imagen (probado y descartado en E-B1) |
| R3 | ADR 0034, ADR 0048 («cada octante»), F2 del plan 2b | La CPU estima por octante lo que la GPU calcula por celda | La CPU calcula por nodo un rango conservador `[lo, hi]` solo para pedir, retener y soltar niveles. [ADR 0055](../adr/0055-la-gpu-decide-el-nivel-la-cpu-la-residencia.md) (propuesta) |
| R4 | Plan 2b: modo octante sin cambios byte a byte | Mantiene dos rutas (27 ramas `LodMode` en `cell_pool.rs`) y tiene sus propios huecos ([#59](https://github.com/Mili-ssm/OctreeViewer/issues/59)) | El modo octante queda congelado: no recibe arreglos y deja de condicionar el diseño del modo celda. Retirarlo es decisión del propietario (Q2) |
| R5 | ADR 0053 (contiguo, ~531 MiB) frente a D4 y al pool de 512 MiB | 531 MiB es el peor caso estimado en la fase 0a con todos los niveles | Contiguo dentro del presupuesto; si no cabe, el ajuste sube el objetivo de px de toda la nube (I4), como ya dice R3-Q11. Sin cifra nueva hasta medirlo |
| R6 | Puertas por frame y de bordes como criterio de parada | No discriminan | La comprobación que decide es el navegador en las poses P1–P5; las puertas de simulación se miden y se informa su cobertura visible |
| R7 | R3-Q13 habla de celdas «por encima del objetivo»; RQ-09 ya dice «fuera de la regla» | El ADR 0054 invirtió el sentido y R3-Q13 quedó con el antiguo | Corregir el texto cuando el propietario valide; sin cambio de cifra |

Sin cambios: RF-03, RF-04, RQ-01 a RQ-05, ADR 0050, 0052 y 0054, la regla E4 de la GPU, la máscara entre nodos y
la puerta D5, y la maquinaria de carga (R3-Q19, R3-Q20, ajuste al pool).

## Experimentos

Ramas locales del checkout principal, sin publicar. Ninguna se fusiona sin el visto bueno del propietario.
`explore/cell-active-run` contiene a `explore/2b-contiguous-active` y la versión vigente de este documento.

| Rama | Cambio | Tests (`--profile fast --lib`) | Simulación real (Belvedere V7, modo celda) | Navegador (muestra integrada) |
| --- | --- | --- | --- | --- |
| `feat/cell-lod-2b` (base) | — | render 400/400, wasm 178/178 | órbita s1: 2166 cambios de nodo, 0 dentro del nodo, bordes 2627/17741; órbita s7: 343, 0, 1857/12267 | P2 con el cuadrante inferior izquierdo negro |
| `explore/2b-contiguous-active` (E-A, 39a9c33) | El conjunto activo llega sin huecos hasta el nivel más fino dibujado (`cell_active_depths`); test de la pose P2 | render 400/400, wasm 179/179; el test nuevo falla antes y pasa después | 6/6 pasan; contadores sin registrar | P2 relleno: píxeles iluminados abajo-izquierda 12,4 % → 20,9 %; P1, P3 y P4 sin cambios; consola limpia |
| `explore/cell-active-run` (E-B1, 444d385) | Activo = niveles dibujados más los residentes del rango que los alcanzan sin hueco; se borra la ventana de ±1 nivel por frame (−52 líneas) | wasm 179/179; render 399/400 (ver nota) | 6/6 pasan; órbita s1: 2166, 0, 2627/17741; órbita s7: 343, **280**, 1811/12267 | Igual que E-A en reposo ([detalle](#resultado-en-el-navegador-de-e-b1)) |

- **Nota del test rojo de E-B1:** `pick_on_driver_records_equals_the_twin_after_retained_frames` exige que algún
  frame retenido mueva el conjunto activo, cosa que solo producía la ventana borrada. Los picks coinciden con el
  gemelo. Hace falta otro escenario (un nivel que llega durante un frame retenido); no se ha aflojado el test.
- **Variante descartada en E-B1:** anclar el tramo en el nivel residente más grueso. Cuatro simulaciones sintéticas
  fallaron con «went from depth 10 to 7 (wanted 8)»: al llegar un nivel grueso con hueco, la imagen engrosaba.
- **Lectura:** E-A es un arreglo pequeño y seguro, candidato a entrar en la fase 2b tal cual. E-B1 muestra el coste
  de quitar la ventana: 280 saltos de más de un nivel dentro del nodo en una órbita (antes 0) a cambio de menos
  estado. Que ese coste se vea o no en pantalla lo decide el navegador y el propietario (Q1).
- Evidencia: capturas en `.superpowers/sdd/replanteamiento/` del checkout principal (ignorado por git).

### Resultado en el navegador de E-B1

- En reposo, P1–P4 son iguales a E-A píxel a píxel (diferencia media de luma ≤ 0,01; los binarios wasm difieren).
  En P3, 137 motas de ≤ 4 px en ambos. Consola sin errores ni avisos.
- En la secuencia de zoom P5, la fracción de píxeles iluminados por frame coincide con E-A salvo un frame de
  desfase de captura, sin caídas bruscas. Las capturas de P5 no se han revisado a ojo: si quitar la ventana
  produce saltos visibles en movimiento **queda sin verificar** y es lo que el propietario debe mirar en la demo.

## Tareas

- [x] **T1 (E-A)** Conjunto activo contiguo hasta el nivel más fino dibujado. Test
  `a_node_holding_the_eye_keeps_contiguous_active_depths_and_no_sub_pixel_cells`: falló antes («node 14 active
  [12, 17] … [13, 14, 15, 16] missing») y pasa después; batería sin regresiones.
- [x] **T2 (E-B1)** Activo anclado en lo dibujado, sin ventana. Un test reescrito
  (`a_node_activates_the_contiguous_resident_run_of_its_range`, sustituye al de la ventana) y un rojo conocido (nota).
- [x] **T3** Navegador en P1–P5 en las dos ramas (guion `run.js` de la evidencia de la fase 2b). Sin revisar a ojo: la secuencia P5 de E-B1.
- [ ] **T4** `scripts/check.sh` completo: no ejecutado en esta sesión (sí clippy y los tests de los dos crates).

Fuera de esta sesión, si el propietario valida la dirección (orden propuesto):

1. Sustituir en modo celda las sondas por octante por el rango por nodo (borra `wanted`/`drawn`/`stage`/`step`,
   las reglas de octantes detrás del ojo, `node_far_depth` y las semillas).
2. Sustituir la retención por nodo (R3-Q21) y la semilla de entrada por una puerta de residencia en los bordes:
   un hijo tapa a su antepasado cuando su tramo alcanza el nivel más fino del antepasado más uno.
3. Medir bytes residentes y tiempo del paso de celdas en Majadas (rasante y dentro de una hoja).
4. Decidir la cola bajo la separación (Q3).

## Preguntas para el propietario

| # | Pregunta | Recomendación |
| --- | --- | --- |
| Q1 | ¿«Un nivel por frame» es literal o el fin es que no se vean saltos? | El fin es la suavidad: R1 |
| Q2 | ¿Se retira el modo octante y el modo celda pasa a ser el predeterminado de la librería? | Sí, tras validar la fase 2b en el navegador |
| Q3 | En las poses P3 y P4, cerca del ojo, cada punto es un cubo de 4–8 px con huecos entre ellos (la muestra tiene un punto cada ~0,13 unidades). ¿Es el aspecto de «microvóxel» que quieres, o los cubos no deberían bajar del espaciado de la nube? | Probar el filtro de longitud «espaciado de la nube» (RF-05, sin implementar) antes de decidir |
| Q4 | Si el tramo contiguo no cabe en 512 MiB, ¿sube el objetivo de px de toda la nube (R5) o se amplía el pool? | Subir el objetivo de px; medir antes |
| Q5 | ¿Se acepta el ADR 0055 y se revisan RF-31, R3-Q21 y el texto de R3-Q13 como en la tabla (R1, R2 y R7)? | Sí |
