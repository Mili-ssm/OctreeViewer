# Requisitos del producto

**Estado:** acordados con el propietario el 30-10-2026 (entrevista de 13 rondas). Documento **vivo**:
fuente de verdad de requisitos; las decisiones con alternativas están en los ADR 0018–0026. Un
cambio de requisito se anota aquí con fecha y, si cambia una decisión, con un ADR nuevo.

## Propósito y usuarios

Visor de nubes LiDAR en el navegador (Rust, WASM, WebGPU) y conversor nativo/web a un formato
jerárquico propio, para navegar cientos de GB de LAZ con memoria acotada, cubos de 1–8 px en
pantalla y carga continua (sin saltos a grueso, huecos ni bloqueos de frame).

| Usuario | Necesidad |
| --- | --- |
| Integrador | Librería embebible: construir aplicaciones sobre una API pública componible. |
| Usuario de la app web pública | Abrir nubes locales o remotas sin instalar nada. |
| Propietario / investigación | Sus datasets y equipos; evidencia medida. |

No es objetivo una demostración técnica sin uso real ni convertir cientos de GB en el navegador.

**Prioridad cuando chocan:** fidelidad > fluidez > escala > memoria > velocidad de conversión.

## Escenarios de referencia

| Id | Escenario | Rol |
| --- | --- | --- |
| ESC-1 | Una nube de **400–500 GB de LAZ** (~10¹¹ puntos, ASM-01) | Puerta de escala |
| ESC-2 | Varias nubes simultáneas de 50–200 GB, **hasta ~100 nubes**, total **< 1 TB** de LAZ | Referencia de uso, no límite |
| EQ-REF | GPU o iGPU con **≥ 4 GB** disponibles para WebGPU + **16 GB de RAM** (p. ej. Radeon 880M con su memoria compartida configurada a ≥ 4 GB), Chrome/Edge de escritorio. Sustituye a los ~2 GB de P2 (05-11-2026) | Equipo de las puertas |
| RED-REF | **100 Mbit/s, 50 ms de RTT** simulados | Red de las puertas remotas |

Demostración (RQ-11): un dataset real ≥ 50 GB y una réplica sintética en mosaico hasta ESC-1, en EQ-REF.

## Requisitos funcionales

### Visor

| Id | Requisito (EARS) | Fuente |
| --- | --- | --- |
| RF-01 | El visor deberá cargar y dibujar a la vez varias nubes convertidas, leyendo de varios archivos simultáneamente, con un único presupuesto de memoria y GPU. | Propietario |
| RF-02 | Si se carga una nube cuyo CRS difiere del de las nubes abiertas, entonces el visor deberá rechazarla con un mensaje que nombre ambos CRS. | Propietario, ADR 0021 |
| RF-03 | El visor deberá dibujar cada celda como un **cubo 3D** alineado con la rejilla global. | P1, ADR 0019 |
| RF-04 | Mientras el modo de huecos sea **literal** (predeterminado), el visor no deberá dibujar cubos donde la fuente no tiene puntos. | P4 |
| RF-05 | Donde el usuario active la **continuidad** en una nube, el visor deberá aplicar a toda esa nube un tamaño mínimo de cubo derivado de su densidad global (no por región). | P4 |
| RF-06 | Mientras el modo de px sea **fijo** (predeterminado), el visor deberá refinar de forma progresiva hacia el objetivo elegido, dibujando lo residente sin esperar a completarlo. | P5 |
| RF-07 | Donde el modo de px **dinámico** esté activo, el visor deberá ajustar el objetivo de px para sostener 16,7 ms por frame, con histéresis. | P5 |
| RF-08 | Mientras la cámara se mueve, el visor deberá reconstruir con la última vista y dibujar la escena anterior hasta que llegue la nueva. | P7, ADR 0018 |
| RF-09 | El visor deberá leer datasets locales (`Blob`) y remotos (HTTP Range). | P6, ADR 0023 |
| RF-10 | El visor deberá ofrecer medición (distancias, alturas, áreas), filtro por clasificación y rangos de atributos, recorte por caja o plano y exportación de la selección a LAS/LAZ. | Propietario |

### API pública

| Id | Requisito (EARS) | Fuente |
| --- | --- | --- |
| RF-11 | Cada función del visor deberá estar disponible a través de la API pública; la app de demostración deberá usar solo esa API. | Propietario, ADR 0026 |
| RF-12 | La API deberá exponer operaciones y estado (nubes, cámara, modos de px, filtros, recorte, medición, exportación). | Propietario |
| RF-13 | La API deberá emitir eventos de progreso de carga, picking, cambios de cámara, errores y métricas. | Propietario |
| RF-14 | La API deberá permitir consultar puntos y atributos de una región o bajo el cursor. | Propietario |
| RF-15 | La API deberá permitir capas de render propias en el mismo canvas WebGPU. | Propietario |

### Conversor

| Id | Requisito (EARS) | Fuente |
| --- | --- | --- |
| RF-20 | El CLI deberá aceptar una lista de archivos LAS/LAZ o una carpeta. | Propietario |
| RF-21 | Cuando reciba varias entradas, el CLI deberá, según una opción, fusionarlas en una nube o generar una nube por archivo. | Propietario |
| RF-22 | Donde se indique un CRS de destino, el CLI nativo deberá reproyectar las entradas a ese CRS. | Propietario, ADR 0021 |
| RF-23 | El conversor deberá cuantizar sobre una **rejilla global determinista**: dos nubes con el mismo CRS, convertidas en cualquier lugar o momento, deberán producir celdas coincidentes en todos los niveles. | Propietario, ADR 0021 |
| RF-24 | El formato deberá conservar CRS (WKT/GeoTIFF), flags LAS (withheld, synthetic, keypoint, overlap) y Extra Bytes, y la exportación deberá devolverlos. Waveform queda fuera. | Propietario, ADR 0022 |
| RF-25 | Si el disco temporal estimado no cabe en el destino, entonces el CLI deberá rechazar la conversión antes de empezar. | Propietario |
| RF-26 | El CLI deberá identificar la fuente de un checkpoint por ruta, tamaño y `mtime` y, donde se pase `--verify-source`, también por hash del contenido. | Propietario |
| RF-27 | Si el esquema supera 256 atributos, entonces el escritor deberá rechazar la conversión con un mensaje claro. | Propietario, ADR 0007 |
| RF-28 | La UI de conversión web deberá anunciar «validado hasta Majadas (~6 GB de LAZ)» y mostrar la estimación de RAM y OPFS antes de empezar; lo mayor se deriva al CLI. | Propietario |
| RF-29 | Si en Firefox la conversión necesita más cuota de la concedida sin `persist()`, entonces la UI deberá pedir `persist()` y explicar el límite. | W2 |

## Requisitos de calidad

| Id | Requisito | Puerta |
| --- | --- | --- |
| RQ-01 | Intervalo entre respuestas de frame del worker al hilo principal: **p95 < 33 ms y máximo < 100 ms** durante la carga y una órbita de 10 s. | ESC-1, EQ-REF |
| RQ-02 | La etapa LOD publicada nunca sube durante una órbita; sin aumento de nodos sin cubrir ni píxeles vacíos frente a la línea base. | Cada cambio de render |
| RQ-03 | En local: **primer frame ≤ 5 s** y **objetivo de 4 px en la vista inicial ≤ 60 s**. | ESC-1, EQ-REF |
| RQ-04 | En remoto: **primer frame ≤ 10 s** y **objetivo de 4 px ≤ 120 s**. | ESC-1, RED-REF |
| RQ-05 | PSS de la pestaña del visor **≤ 8 GiB**. | ESC-1 y ESC-2, EQ-REF |
| RQ-06 | Disco temporal del CLI **≤ 2 × el tamaño del LAZ de entrada** (objetivo; ASM-03). | Conversión ESC-1 |
| RQ-07 | V7 final frente a V5: latencia de lectura y navegación igual o mejor; conversión con tiempo ≤ 2,1× y RSS pico ≤ +40 % ([ADR 0043](adr/0043-coste-de-conversion-de-v7-frente-a-v5.md); era ≤ +30 % hasta el 15-11-2026); disco, solo lo que añada la pirámide densa (+15–35 % estimado). | Promoción de V7 |
| RQ-08 | La conversión web produce los mismos bytes que el CLI (referencias R1). | Cada cambio del conversor |
| RQ-09 | Ningún cubo por encima del objetivo de px salvo lo que imponga la densidad de la fuente ([contrato](explanation/REGIONAL_VOXEL_LOD.md)). Con la cámara dentro del dataset, la ruta de celdas admite ≤ 0,5 % de celdas en pantalla por encima ([ADR 0034](adr/0034-nivel-por-octante-con-arista-real.md)). | Cada cambio de render |
| RQ-10 | Los controles de la app pública cumplen **WCAG 2.2 AA** (teclado completo, foco visible, contraste, etiquetas), verificado con axe y Playwright. El lienzo 3D queda fuera. | App pública |
| RQ-11 | La escala se demuestra con un dataset real ≥ 50 GB y una réplica sintética hasta ESC-1, midiendo RQ-01 a RQ-05. | 1.0 |

## Restricciones

| Id | Restricción | Fuente |
| --- | --- | --- |
| CON-01 | Navegadores: **Chrome/Edge de escritorio** con puertas. Firefox de escritorio, **mejor esfuerzo** (se conserva lo que funciona y se informa de sus límites, sin puertas). Safari y móvil fuera de alcance. | Propietario |
| CON-02 | No se exige COOP/COEP; si el sitio las sirve, se usan hilos wasm con memoria compartida. | Propietario, ADR 0024 |
| CON-03 | Licencia **MIT OR Apache-2.0**. | Propietario, ADR 0026 |
| CON-04 | La API es un paquete npm en TypeScript tipado; los crates de Rust son internos, sin descartar publicarlos en el futuro. | Propietario, ADR 0026 |
| CON-05 | Nada se publica (npm ni app pública) hasta la **1.0**: RQ-11 superado, V7 final congelado y API con RF-10 a RF-15, con la demo construida solo sobre ella. | Propietario, ADR 0026 |
| CON-06 | La UI y los mensajes de la API, solo en inglés. | Propietario |
| CON-07 | Nightly con fecha fija se mantiene: los hilos wasm opcionales necesitan `build-std` con atomics. | Propietario, ADR 0011, ADR 0024 |
| CON-08 | La conversión de cientos de GB solo es nativa (CLI); en el navegador, hasta Majadas. | Propietario |
| CON-09 | El codificador LZ4 es propio, en `viewer_format`. | Propietario, ADR 0025 |
| CON-10 | Siguen vigentes: pareja `.oct`/`.octattrs` (ADR 0001), núcleo de conversión sin I/O (ADR 0014), norma portátil (ADR 0016). | ADR |

## Supuestos (sin verificar)

| Id | Supuesto | Se comprueba en |
| --- | --- | --- |
| ASM-01 | 500 GB de LAZ son ~10¹¹ puntos. | Dataset real de RQ-11 |
| ASM-02 | Una vista de 4 px a 1080p necesita decenas de MB con la pirámide densa. | R5 (remoto) |
| ASM-03 | ≤ 2 × LAZ de temporales es alcanzable con spools comprimidos y fusión por fases. | Spike de temporales del CLI |
| ASM-04 | Las nubes reales tienen menos de 30 atributos. | Corpus de RQ-11 |

## Orden de trabajo acordado

1. W1: la etapa 4 está integrada en `master`, sin evidencia registrada del chequeo previo a la integración.
2. Escala: R4 (pirámide densa y rejilla global) → R3 (cubos en GPU; [ADR 0028](adr/0028-r4-antes-que-r3.md), 01-11-2026) → R5 (jerarquía paginada y
   remoto) → congelar V7 final ([plan de render](plans/render-y-carga-escala.md)).
3. API pública y funciones RF-10 a RF-15.
4. Conversor web: M6 (Majadas) con un worker; M4/M5 solo si M6 supera 60 min o la experiencia lo pide.

## Resoluciones del 30-10-2026

Opciones y recomendación originales de cada pregunta: `git show 10e5074^:docs/explanation/preguntas-propietario.md`.

| Pregunta | Resolución | Registro |
| --- | --- | --- |
| W1 | Fusionar tras `check.sh`; etapa 4 integrada, sin evidencia registrada del chequeo previo. | Orden de trabajo |
| W2 | Pedir `persist()` con mensaje; Firefox es mejor esfuerzo. | RF-29, CON-01 |
| P1 | Cubos 3D obligatorios. | ADR 0019 |
| P2 | ~~iGPU 2 GB + 16 GB RAM.~~ Sustituida el 05-11-2026: ≥ 4 GB de GPU. | EQ-REF |
| P3 | Pirámide densa en V7. | ADR 0020 |
| P4 | Literal por defecto; continuidad opcional por nube. | RF-04, RF-05, ADR 0019 |
| P5 | Fijo progresivo por defecto; dinámico a 60 fps. | RF-06, RF-07, ADR 0019 |
| P6 | Local y remoto; COOP/COEP opcional. | ADR 0023, ADR 0024 |
| P7 | Última vista; se dibuja la escena anterior. | ADR 0018 |
| P8 | Worker de construcción por defecto. | ADR 0018 |
| Volumen | LAZ; ESC-1 y ESC-2. | Escenarios |
| Rejilla y CRS | Rejilla global determinista; el CLI reproyecta y el visor rechaza la mezcla. | ADR 0021 |
| Orden de V7 final | Incluye R4, R5 y la rejilla global. | ADR 0022 |
| LZ4 | Codificador propio. | ADR 0025 |
| Límite de atributos | El escritor rechaza más de 256. | ADR 0007 |
| Plan de render | Cancelar R1-T2; R2b absorbida por R4; R1-T6 dentro de R5; R1-T1b pasa a modos de px. | [Plan de render](plans/render-y-carga-escala.md) |
| Pool web | Medir M6 con un worker antes de M4/M5. | [Spec](plans/conversor-web-paralelo.md) |
| Potree | Paridad cualitativa; abierta a métricas. | OQ-02 |

## Resoluciones del 01-11-2026

| Pregunta | Resolución | Registro |
| --- | --- | --- |
| Orden de escala | R4 antes que R3. | ADR 0028 |
| R4-Q1 a R4-Q4 | Coordenadas enteras globales de cuanto 2⁻¹⁵ con octree implícito en sus bits (ADR 0029); límite de hoja medido (16/32/64 k); espaciado ÷2 por nivel; PROJ fuera de `v7.4-pre`. | [Plan de R4](plans/render-r4-piramide-densa.md#resoluciones-del-propietario-01-11-2026) |

## Resoluciones del 05-11-2026

| Pregunta | Resolución | Registro |
| --- | --- | --- |
| GPU mínima | GPU o iGPU con ≥ 4 GB disponibles para WebGPU; sustituye a P2. | [ADR 0033](adr/0033-gpu-minima-4-gb-y-corte-ajustado-al-pool.md), EQ-REF |
| R3: hojas, claves e instancia | Continuación adaptativa de hojas, clave regional más allá de 21 bits e instancia de 12 B. | ADR 0032, R3-Q7 y R3-Q8 |
| R3: hoja separada | No se refina más; entra en la excepción «densidad de la fuente» de RQ-09. | R3-Q9 |
| R3: saltos de pose | Con `DescendantTooFine` se retiene el corte anterior hasta completar el nuevo. | ADR 0033, R3-Q10 |
| R3: corte mayor que el pool | Los nodos más lejanos bajan a un nivel más grueso, como etapa contada, hasta que el corte quepa. | ADR 0033, R3-Q11 |
| R3: error proyectado | Nivel por octante con la arista proyectada real; ≤ 2× la CPU a 2–8 px y ≤ 2,5× a 1 px en la pose rasante. | ADR 0034, R3-Q12 |
| RQ-09 con la cámara dentro | Tolerancia de ≤ 0,5 % de celdas en pantalla por encima del objetivo. | ADR 0034, R3-Q13 |
| R3: instancia | 16 B con W = 16, en lugar de 12 B con W = 10. | [ADR 0035](adr/0035-instancia-de-16-b-con-w-16.md), R3-Q14 |
| R3: compactación y oclusión | Tarea 16 obligatoria antes de la 6; raymarching solo si no basta. | ADR 0035, R3-Q15 |
| R3: técnica de oclusión | Hi-Z por celda en dos fases, con caja ajustada por página; el Hi-Z solo por página no aporta. | [ADR 0036](adr/0036-hi-z-por-celda.md), R3-Q16 |
| R3: coste de CPU del pool | La regla de 2 ms por frame decide a 4 px; Majadas a 1 px es prueba de carga. | R3-Q17 |
| R3: nivel por octante en movimiento | Hasta ~5 % por encima del objetivo de px con la cámara en movimiento; exacto con la cámara quieta. | R3-Q18 |
| R3: aplicar un corte de celdas | Sin huecos y con retención local: el antepasado dibuja la zona de un nodo hasta que el nivel del nodo está en GPU, y lo que falta en una zona no bloquea las demás. Carga de grueso a fino. | R3-Q19 |
| R3: nodos nuevos sin respaldo | Se precarga su nivel más grueso y entran al ser residentes, sin bloquear el resto; al alejar, los descendientes que ya se dibujaban cubren la zona hasta entonces; se retiene el corte anterior solo si la zona no se puede cubrir. | R3-Q20 |
| R3: la imagen solo se refina | El antepasado dibuja la zona de un hijo hasta que el hijo dibuja ahí un nivel al menos tan fino como el del antepasado o el pedido, el más grueso de los dos, por nodo entero; dentro de un nodo se dibuja el residente más cercano al deseado. | R3-Q21 |
| R3: padres primero | Antes de construir un nodo se construye su padre; la escalera de px solo arranca al abrir o tras un salto de cámara, no al orbitar. | R3-Q22 |
| R3: celdas y modos de color | Hasta la Tarea 12, otro modo de color vuelve a la ruta CPU y volver a RGB reactiva las celdas. | R3-Q23 |
| R3: celdas sin worker de construcción | Sin el worker de construcción se usa la ruta CPU y `cellPath` lo indica (`needsBuildWorker`). | R3-Q24 |

## Resoluciones del 15-11-2026

| Pregunta | Resolución | Registro |
| --- | --- | --- |
| C4 / RQ-07: coste de conversión de V7 | Se revisa la cifra de conversión de [RQ-07](#requisitos-de-calidad); TC5 no se planifica. | [ADR 0043](adr/0043-coste-de-conversion-de-v7-frente-a-v5.md) |
| Congelado de V7 (D3c) | V7 final sobre v7.5-pre, escritor híbrido fijo, salida predeterminada ya; checkpoint y EPT exigen V5; `-pre` conservadas; LZ4 propio después. | [ADR 0044](adr/0044-v7-final-congelada-y-predeterminada.md) |
| Integración de `feat/r3-v75` | Remoto `origin` = `github.com/Mili-ssm/OctreeViewer` (privado); su historial de 2023 no tiene relación con este y se sobrescribe; la integración va por PR. | [Plan](plans/mejoras-pendientes-13-11-2026.md#cómo-retomar-15-11-2026) |

## Catálogo: funcional o técnico, duro o blando (propuesta 14-11-2026)

**Propuesta pendiente de revisión del propietario.** Funcional: lo que el producto debe lograr (el fin). Técnico: el medio elegido. Duro: necesidad real (usuario, producto, legal, seguridad o resultado fijado por el propietario) que solo cambia con una decisión suya. Blando: medio elegido o medida temporal que se puede abandonar cuando la ganancia supere el coste; «Revisar si» da la condición. «Por confirmar» marca la dureza dudosa. Los ASM-* son supuestos, no requisitos: no se clasifican. Los umbrales numéricos de los RQ son objetivos medibles de un fin duro; su cifra concreta es revisable.

| ID | Funcional o técnico | Duro o blando | Por qué | Revisar si |
| --- | --- | --- | --- | --- |
| RF-01 | Funcional | Duro | Caso de uso central (varias nubes) | — |
| RF-02 | Funcional | Duro | Evita mezclar geometrías incoherentes (ADR 0021) | — |
| RF-03 | Funcional | Duro | P1: cubos 3D obligatorios | — |
| RF-04 | Funcional | Duro | Fidelidad: no inventar datos | — |
| RF-05 | Funcional | Blando | Opcional por nube; mejora visual | Un modo mejor lo sustituya |
| RF-06 | Funcional | Por confirmar | Es el modo por defecto, ¿o el comportamiento es la necesidad? | El propietario decida el modo por defecto |
| RF-07 | Funcional | Blando | Opcional; 16,7 ms es la meta de fluidez | Cambie la meta de fotogramas |
| RF-08 | Técnico | Blando | Medio para no bloquear el frame (ADR 0018) | Otra estrategia dé la misma fluidez |
| RF-09 | Funcional | Duro | Nubes locales y remotas (P6) | — |
| RF-10 | Funcional | Duro | Funciones de producto | — |
| RF-11 | Funcional | Duro | Integrador, resolución del propietario (ADR 0026) | — |
| RF-12–RF-15 | Funcional | Duro | Superficie de la API para el integrador | — |
| RF-20, RF-21 | Funcional | Duro | Entradas y fusión del conversor | — |
| RF-22 | Funcional | Duro | Mezclar CRS exige reproyección (ADR 0021) | — |
| RF-23 | Técnico | Duro | Resolución del propietario; sustenta RF-02 y la fusión | Otra técnica dé celdas coincidentes |
| RF-24 | Funcional | Duro | Sin pérdida de metadatos LAS (ADR 0022) | — |
| RF-25 | Funcional | Por confirmar | Protege al usuario de fallos tardíos; ¿requisito o salvaguarda? | — |
| RF-26 | Técnico | Blando | Mecanismo de identidad de checkpoint | Otro mecanismo detecte fuentes cambiadas |
| RF-27 | Técnico | Blando | Límite del formato (ADR 0007) | El formato admita más atributos |
| RF-28 | Funcional | Blando | Refleja el alcance validado hoy (Majadas) | Se valide un tamaño mayor |
| RF-29 | Técnico | Blando | Apaño del límite de cuota de Firefox | Firefox deje de ser mejor esfuerzo o cambie la cuota |
| RQ-01–RQ-05 | Funcional | Duro | Fluidez, escala y memoria acotada (propósito) | Cifras: cambie el equipo o la red de referencia |
| RQ-06 | Técnico | Blando | Objetivo sin medir (ASM-03) | El spike de temporales lo mida |
| RQ-07 | Técnico | Blando | Protege coste de conversión; cifra revisada por el propietario (ADR 0043) | Ver «Revisar si» del ADR 0043 |
| RQ-08 | Técnico | Por confirmar | Garantiza reproducibilidad; ¿lo exige el usuario o es comodidad de pruebas? | — |
| RQ-09 | Funcional | Duro | Fidelidad de nivel de detalle; la tolerancia del 0,5 % es blanda (ADR 0034) | La tolerancia cueste más que lo que gana |
| RQ-10 | Funcional | Duro | Accesibilidad de la app pública | — |
| RQ-11 | Funcional | Duro | Demuestra la puerta de escala | — |
| CON-01 | Funcional | Duro | Alcance de plataformas fijado por el propietario | El propietario amplíe el alcance |
| CON-02 | Técnico | Blando | Sin exigir cabeceras del servidor (ADR 0024) | Los hilos wasm pasen a ser necesarios |
| CON-03 | Funcional | Duro | Licencia (legal) | — |
| CON-04 | Técnico | Duro | Resolución del propietario (ADR 0026) | Decisión del propietario |
| CON-05 | Funcional | Duro | Criterio de publicación del propietario | — |
| CON-06 | Funcional | Por confirmar | Sin razón registrada | — |
| CON-07 | Técnico | Blando | Atomics requieren nightly (ADR 0011, 0024) | Estabilice `build-std` con atomics |
| CON-08 | Técnico | Blando | Límite práctico del navegador | Se valide la conversión web a mayor escala |
| CON-09 | Técnico | Blando | Elección de implementación (ADR 0025) | Una dependencia externa sea mejor |
| CON-10 | Técnico | Por confirmar | Cada ADR citado tiene su propia razón | Un ADR citado se sustituya |
| ~~**Orden de filas de la fuente dentro de cada bloque**~~ (`crates/viewer_format/src/contracts.rs:11,28`; `occupied_point_block.rs`, `occupied_cell_index.rs` en `viewer_core`) | Técnico | Blando | **Retirado** (propietario, 14-11-2026, [ADR 0041](adr/0041-orden-de-filas-libre-por-bloque.md)): se cumplió la condición de revisión; el orden Morton por bloque, cuando reduce la geometría, deja el `.oct` un 35 % más pequeño en Belvedere v7.5 ([registro](records/benchmarks/OQ09_MORTON_ORDER_14-11-2026.md)). La estimación previa (0,63 %) solo contaba quitar el índice de filas | Ver «Revisar si» del ADR 0041 |
| **Instancia de 16 B** (ADR 0035) | Técnico | Blando | Medio de la ruta de celdas | Cambie la compactación o la oclusión |
| **Pool de celdas de 512 MiB** (ADR 0040) | Técnico | Blando | Medio dentro de la GPU mínima de 4 GB | Un adaptador objetivo no lo admita |
| **Presupuesto de CPU de 512 MiB en wasm** (ADR 0037) | Técnico | Blando | Medio para acotar memoria wasm | Se mida que un presupuesto mayor no cuesta memoria |

## Preguntas abiertas

| Id | Pregunta | Por qué se aplaza | Revisar cuando |
| --- | --- | --- | --- |
| OQ-01 | Dónde se publican la app y la librería (GitHub Pages o servidor/CDN propio) | Sin decidir por el propietario | La API cubra RF-10 a RF-15 |
| OQ-02 | Métricas frente a Potree 2 y COPC | Basta paridad cualitativa por ahora | RQ-11 superado |
| OQ-03 | Avisos upstream de seguridad de `las`/`laz` (README, prioridad 9) | Decisión del 28-10-2026: más adelante | Antes de publicar la 1.0 (propuesto) |
| OQ-04 | Techo de temporales definitivo | RQ-06 es un objetivo sin medir (ASM-03) | Spike de temporales del CLI |
| OQ-05 | ~~Compresión de cadenas de nodos con un solo hijo (propietario, 02-11-2026)~~ | Rechazada en [ADR 0030](adr/0030-jerarquia-paginada-por-anclas.md): 6 y 9 nodos con un solo hijo; ahorraría < 6 KB | Más del 1 % de nodos con un solo hijo |
| OQ-06 | ~~Direccionamiento compacto de nodos: nivel `u8` + camino relativo `u16` (5 niveles × 3 bits) con nodos ancla de prefijo completo cada 5 niveles (propietario, 02-11-2026)~~ | Adoptado en [ADR 0030](adr/0030-jerarquia-paginada-por-anclas.md): nodos de página por (profundidad relativa `u8`, camino `u16`) bajo un ancla de prefijo completo | — |
| OQ-07 | Profundidad del visor por encima de 24: claves `u64` en CPU y posiciones relativas a un ancla cercana en GPU (WGSL no tiene `u64`) | Majadas y Belvedere llegan a ~22 y las nubes locales ya tienen raíz ajustada | Reanudar R3 (celdas ancladas por nodo) |
| OQ-08 | No dibujar celdas ocultas: HZB en dos pasadas por página (Tarea 16), raymarching por píxel o un híbrido ([opciones](records/benchmarks/R3_OCCLUSION_OPTIONS_05-11-2026.md); propietario, 05-11-2026) | La Tarea 16a pasa con compactación y Hi-Z por celda en dos fases: Majadas dentro de una hoja a 4 px en 4,39 ms en la peor dirección ([registro](records/benchmarks/R3_T16_OCCLUSION_05-11-2026.md)); el raymarching no se activa (ADR 0035) | La Tarea 16b o la 17 superan 16,7 ms en Majadas dentro de una hoja a 4 px |
| OQ-09 | ~~¿Debe conservarse el orden de filas de la fuente dentro de cada bloque en V7 final? El orden Morton quitaría el índice de fila por punto del códec de ocupación ([revisión, TF1](records/benchmarks/REVIEW_TECHNIQUES_13-11-2026.md#formato))~~ | Resuelta (propietario, 14-11-2026): requisito técnico blando; se mantiene el orden mientras el ahorro medido (0,63 %) no compense el coste ([catálogo](#catálogo-funcional-o-técnico-duro-o-blando-propuesta-14-11-2026)). **Revisada el mismo día** con la medida del orden híbrido (−35 % en el `.oct`, [registro](records/benchmarks/OQ09_MORTON_ORDER_14-11-2026.md)): V7 deja el orden libre por bloque y el escritor elige el más pequeño ([ADR 0041](adr/0041-orden-de-filas-libre-por-bloque.md)) | — |
| OQ-10 | ¿Se prevé un visor nativo o sin cabeza? Decide si la política de LOD y de carga sale de `viewer_wasm` ([revisión, TA4](records/benchmarks/REVIEW_TECHNIQUES_13-11-2026.md#arquitectura-y-patrones)) | Sin necesidad de producto registrada | Al planificar la API pública (ADR 0026) |
