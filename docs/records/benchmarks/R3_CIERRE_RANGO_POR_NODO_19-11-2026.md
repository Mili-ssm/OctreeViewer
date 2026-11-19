# R3: cierre del plan del rango por nodo — 19-11-2026

**Estado: medido; plan cerrado con salida reducida (decisión del propietario).** Los huecos transitorios del
conjunto activo se quedan: el arreglo pequeño que los quitaba (un solo tramo, el del nivel dibujado más fino) se
probó y se revirtió porque empeoraba esas mismas celdas (ver abajo). El cambio mayor previsto (que un tramo único por
nodo, calculado sin los campos por octante, gobierne la emisión, y borrar después la maquinaria por octante) se
intentó y se aparcó: al conectarlo aparecieron reglas que el diseño no tenía y 28 tests en rojo, para un beneficio
visible pequeño. Queda como experimento en la PR en borrador
[#88](https://github.com/Mili-ssm/OctreeViewer/pull/88).

Cierra el [plan del ADR 0055 en modo celda](../../plans/adr-0055-rango-por-nodo.md). Continúa los registros del
[tope por datos](R3_TOPE_POR_DATOS_19-11-2026.md) y de las
[fuentes de la cota](R3_FUENTES_COTA_POR_DATOS_18-11-2026.md).

## Condiciones

| Dato | Valor |
| --- | --- |
| Revisión | `c23c682` (rama `feat/adr-0055-tramo-b`); experimento en `exp/adr-0055-node-run-emission` (`b11f967`) |
| Perfil y toolchain | `fast`; rustc 1.101.0-nightly (2026-10-27) |
| Equipo | AMD Ryzen AI 9 365 |
| Datasets | `target/render-data/belvedere-v7.oct` (simulaciones reales: dolly semilla 3, órbitas 1 y 7, órbita con reinicio semilla 5); simulaciones sintéticas del arnés |
| Comandos | `R3_REQUIRE_REAL_DATA=1 cargo test -p viewer_wasm --profile fast --offline --lib -- --ignored real_sim --nocapture` · `… --ignored real_lag_belvedere_dolly` / `real_lag_belvedere_orbit` |

Las cifras solo valen para estas revisiones, perfil, datasets y equipo.

## Qué cambió en este tramo

| Cambio | Commits | Efecto medido |
| --- | --- | --- |
| Se retira la regla de activos `Window` (propietario, OQ-11) | `d108004`, `f09344a` | Simulaciones con `Contiguous` y entrada de la GPU idénticas; navegador sin cambios |
| Resumen de la entrada de la GPU (bits activos y máscara por nodo) fijado en P1 a P5 y en las simulaciones | `35f69f1` | Determinista en dos ejecuciones |
| El `lo` de un nodo y su cota por datos miden solo octantes con datos a la vista ([#87](https://github.com/Mili-ssm/OctreeViewer/issues/87)) | `5c2f92d`, `c54b129` | Nodo 77 de la órbita 1: rango (12, 12) → (7, 7); antes pedía el nivel 12 de un nodo sin nada en pantalla (0 de 125 muestras del gemelo a la vista) |
| Un tramo activo por nodo (el del nivel dibujado más fino), con `active_gaps == 0` obligatorio | `a123fe3`, `af75572`; **revertidos** en `a1e673c`, `c23c682` | Quitaba los 7 frames con hueco de la órbita 1, pero empeoraba el dibujo (abajo) |

**Por qué se revirtió el tramo único.** La GPU dibuja en cada celda el nivel activo más grueso que no refina, y el
más fino activo dibuja siempre. Con activos {7, 12} (nodo 77), una celda que quiere 7 dibuja 7 y una que quiere 8 a
11 dibuja 12; con solo {12}, **todas** dibujan 12, hasta cinco niveles más fino de lo pedido: celdas subpíxel, el
fallo de «hueco» de un conjunto con salto, ahora en todo el nodo. Mi razonamiento («quitar el tramo grueso solo da
más detalle, nunca menos») miraba solo R3-Q21 y olvidaba que lo demasiado fino también se ve. Los dos tramos son el
conjunto menos malo con la residencia disponible hasta que llegan los niveles que faltan. `active_gaps` vuelve a
ser un recuento (7 frames en la órbita 1, 0 en el resto; el nodo 77 no tiene nada en pantalla tras #87).

Simulaciones reales tras la reversión: las de `step3_sims` de la tarea 10c (contadores a 0; frames hasta asentarse
iguales a la tarea 9: dolly 29, órbitas 53 y 37; 0 muestras dibujables más gruesas).

## El tramo único por nodo como fuente de la emisión: comparación y experimento

Antes de conectarlo se comparó, nodo a nodo y frame a frame, el tramo de `node_run` con lo que la GPU recibía
(`check_run_diff`). Tras tres rondas de ajuste (`5be1a1c`, `8a68d17`, `9b5d027`, `eda80b8`): 0 diferencias sin
clasificar y 0 frames con el tramo por debajo de lo que ya se veía.

Al conectarlo (rama del experimento, último estado):

| Medida | Valor |
| --- | --- |
| Tests `viewer_render_wgpu --lib` | 446 pasan, 19 fallan |
| Tests `viewer_wasm --lib` | 162 pasan, 9 fallan |
| Corte final de las simulaciones sintéticas dolly y dolly con reinicio (semilla 1) | Distinto de la referencia en proceso: depende del historial; causa no encontrada |

Reglas que el diseño no tenía y aparecieron al conectar:

| Hueco del diseño | Regla provisional del experimento |
| --- | --- |
| Con el ancla en el extremo fino anterior del propio nodo, el tramo no baja nunca al alejarse: los niveles sobre `hi` siguen activos y protegidos y no se expulsan | Al alejarse, ancla en el usable más cercano al objetivo (salta al deseo sin bajar nivel a nivel) |
| Con deseos a ambos lados de un nivel que no llega, la espera y el salto rebotan entre los dos extremos y el driver no deja de estar ocupado | Abandonar (`node_step = None`) cuando no hay salto posible |
| El paso hacia lo grueso esperaba un nivel construible pero no almacenado | Servir el deseo desde el primer nivel almacenado, como el código por octante |
| Quitar la espera por octante dejaba atascado el paso por octante | Mantener las dos esperas |

## Límites

- Los huecos transitorios del conjunto activo siguen (contados, no corregidos).
- La maquinaria por octante sigue: pasos de un nivel por frame, semillas de entrada, `step_waits`, el juez
  `PerOctant` (solo en tests) y los campos `wanted`, `drawn`, `stage`, `step`. Junto a ellos, `node_run` y los campos
  por nodo se calculan cada frame; en producción solo los lee la presencia (`claims`: coberturas y nodos ocultos en `fallback.rs` y `plan.rs`), con su equivalencia comprobada en cada frame; el resto solo lo leen los tests.
- Lo aplazado de la tarea 6 (que el oráculo juzgue solo celdas ocupadas, con el rango por nodo como tope) no se hace.
- Un nodo cuyo único nivel usable dentro del rango es el D5 suelto queda fuera de la comparación (visto solo en la
  referencia en proceso).
