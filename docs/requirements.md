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
| EQ-REF | iGPU con ~2 GB compartidos + **16 GB de RAM** (Radeon 880M), Chrome/Edge de escritorio | Equipo de las puertas |
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
| RQ-07 | V7 final frente a V5: latencia de lectura y navegación igual o mejor; tiempo y RSS de conversión ≤ +30 %; disco, solo lo que añada la pirámide densa (+15–35 % estimado). | Promoción de V7 |
| RQ-08 | La conversión web produce los mismos bytes que el CLI (referencias R1). | Cada cambio del conversor |
| RQ-09 | Ningún cubo por encima del objetivo de px salvo lo que imponga la densidad de la fuente ([contrato](explanation/REGIONAL_VOXEL_LOD.md)). | Cada cambio de render |
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
| P2 | iGPU 2 GB + 16 GB RAM. | EQ-REF |
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

## Preguntas abiertas

| Id | Pregunta | Por qué se aplaza | Revisar cuando |
| --- | --- | --- | --- |
| OQ-01 | Dónde se publican la app y la librería (GitHub Pages o servidor/CDN propio) | Sin decidir por el propietario | La API cubra RF-10 a RF-15 |
| OQ-02 | Métricas frente a Potree 2 y COPC | Basta paridad cualitativa por ahora | RQ-11 superado |
| OQ-03 | Avisos upstream de seguridad de `las`/`laz` (README, prioridad 9) | Decisión del 28-10-2026: más adelante | Antes de publicar la 1.0 (propuesto) |
| OQ-04 | Techo de temporales definitivo | RQ-06 es un objetivo sin medir (ASM-03) | Spike de temporales del CLI |
| OQ-05 | Compresión de cadenas de nodos con un solo hijo: el primer nodo de la cadena apunta al primer descendiente con varios hijos (propietario, 02-11-2026) | Tras la rejilla local de las nubes sin CRS, solo quedan cadenas en nubes con CRS que cruzan una potencia de 2 grande; cambia el validador de jerarquía y la transición LOD | La Tarea 6 de R4 mida nodos de cadena y bytes de índice; diseño de `v7.5-pre` |
| OQ-06 | Direccionamiento compacto de nodos: nivel `u8` + camino relativo `u16` (5 niveles × 3 bits) con nodos ancla de prefijo completo cada 5 niveles, que también acotan la compresión de OQ-05 (propietario, 02-11-2026) | Con el índice cargado entero el prefijo se deriva del padre en O(1) y el ahorro es ~21 B por nodo | Diseño de la jerarquía paginada (R5): cada página empezaría en un ancla |
| OQ-07 | Profundidad del visor por encima de 24: claves `u64` en CPU y posiciones relativas a un ancla cercana en GPU (WGSL no tiene `u64`) | Majadas y Belvedere llegan a ~22 y las nubes locales ya tienen raíz ajustada | Reanudar R3 (celdas ancladas por nodo) |
