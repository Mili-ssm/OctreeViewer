# 0048 El render dibuja siempre el nivel que pide la vista y lo construye a demanda si no está guardado

Estado: Aceptada · Fecha: 15-11-2026 · Decide: propietario (la regla; el mecanismo, el agente) · Sustituye la decisión y la revisión del [ADR 0046](0046-la-poda-de-niveles-no-limita-el-render.md); precisa los ADR [0031](0031-celdas-por-nodo-en-v7-5.md) y [0034](0034-nivel-por-octante-con-arista-real.md) y la resolución R3-Q9

## Contexto
- El ADR 0046 solo deriva el nivel pedido cuando es más grueso que **todos** los guardados. Su revisión añade
  que, si hay un guardado más grueso, se dibuja ese. Eso contradice el ADR 0031 (se usa el guardado más fino
  siguiente) y rompería RQ-09. Además deja sin cubrir los huecos en mitad de la cadena podada: un octante que
  pide un nivel podado dibuja uno o varios niveles más finos que sus vecinos.
- R3-Q9: una hoja con todas sus posiciones separadas no se refina más. El propietario (15-11-2026) prefiere
  que el render siga refinando por debajo del espaciado: «no se ve mal y así es coherente con zooms muy
  fuertes».
- El worker de construcción ya calcula cada nivel de la cadena desde los puntos del nodo antes de podarla
  (`node_cells.rs`, `PointNodeCellSource::node_cells`). El nivel empaquetado solo guarda medias (color,
  intensidad) y una clase, así que agrupar celdas empaquetadas pondera mal.

## Decisión
- Cada octante dibuja **el nivel que pide la vista**: el más grueso cuya arista proyectada cumple el objetivo
  de px, acotado por el tamaño mínimo que elija el usuario. Ese nivel es una profundidad cualquiera, no solo
  una de la cadena.
- Si la cadena no tiene ese nivel, el worker de construcción lo **construye a demanda desde los puntos del
  nodo**, con la misma agregación que los niveles guardados. Sirve tanto para niveles podados como para
  niveles más finos que el último de la cadena (refinar por debajo del espaciado), hasta el límite que ya aplica
  la cadena (`MAX_CELL_DEPTH` en `node_cells.rs`; superarlo es un error, como en el ADR 0031).
- Mientras el nivel pedido no está residente, se dibuja lo que ya lo está, como hoy (R3-Q21: la imagen solo
  se refina).
- La poda de la cadena sigue siendo una decisión de almacenamiento: qué niveles se construyen y envían por
  adelantado.

## Alternativas
| Opción | Por qué no |
| --- | --- |
| Agrupar celdas del nivel empaquetado más fino en el worker de render (ADR 0046) | Medias sin ponderar por puntos; no permite refinar por debajo del último nivel |
| Dejar de podar | Más memoria y transferencia (+96 % de celdas en el caso de #35) para resolver un problema de dibujo |
| Mantener R3-Q9 (no refinar una hoja separada) | El propietario prefiere seguir refinando con zooms fuertes |

## Consecuencias
- Hace falta un mensaje nuevo entre el worker de render y el de construcción para pedir un nivel (nodo,
  profundidad) fuera de la cadena, y la ruta en proceso equivalente. Cambian la cabecera de `build-worker.js`,
  `viewer-worker.js` y `tests/cells-protocol.test.mjs`.
- El corte y el pool asumen hoy que el nivel deseado está en la cadena (`expect` y búsquedas en `stored` en
  `cell_cut.rs`, `cell_pool.rs` y `cell_driver.rs`). Esas invariantes cambian, y los niveles construidos a demanda
  cuentan en los presupuestos del pool (R3-Q11, R3-Q17).
- El nivel se construye desde los bloques del nodo: si la caché del worker de construcción los expulsó, hay que
  volver a leerlos.
- El nivel pedido puede tardar más en llegar que hoy: hasta entonces se dibuja el residente.
- Cambian los dorados del corte (`cell_cut_digest`) y los tests que fijan «el siguiente guardado más fino».
- Implementación en [#35](https://github.com/Mili-ssm/OctreeViewer/issues/35), con plan propio.

## Revisar si
Construir niveles a demanda cuesta más tiempo de frame o de espera que guardarlos, medido en Majadas y
Belvedere, o los bloques del nodo ya no están en la caché del worker de construcción cuando se piden.
