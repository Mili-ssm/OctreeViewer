# Diagnóstico del digest en reposo con el worker de construcción (30-10-2026)

Contexto: en Majadas 4 px la sonda `probe-orbit-load.mjs` (esquema 3, `sceneDigestAtRest`) dio digests distintos: OFF `34ea6855b39d50d0`, ON `1f1652bb0a8161ec` (idénticos en 2 ejecuciones por brazo; en la muestra pequeña ambos `a6323f80503602ce`). Véase [la medición del worker](RENDER_BUILD_WORKER_2026-10-30.md).

## Conclusión

**No es un defecto del worker de construcción.** El primer campo divergente es la **cámara** con la que se construyó la escena en reposo: en OFF no era la pose ajustada, sino otra distinta. La causa es la sonda (y el protocolo de entrada del host), no el pipeline de escena. Confianza: alta (causa reproducida y corregida; ambos brazos dan el mismo digest tras el arreglo).

## Evidencia

Instrumentación: `apiCall("sceneInputs")` (viewer-worker.js) devuelve la cámara (11 f64), viewport, límites de voxelización, `targetPx`, modo de color, época, rango de atributo y las estadísticas LOD completas; la sonda lo vuelca al inicio (`sceneInputsAtStart`) y en reposo (`sceneInputsAtRest`).

| Campo | OFF en reposo | ON en reposo | Ambos al inicio |
| --- | --- | --- | --- |
| viewport | 1280×720 | 1280×720 | 1280×720 |
| `voxelBuildLimits`, `targetPx`, modo de color | iguales | iguales | |
| `visualScale` | del dataset | 0,0788… (el builder lo valida contra el dataset) | |
| **cámara, yaw / pitch** | **5,1247 / 1,0690** | **0 / 0,78540** | 0 / 0,78540 |
| digest | `34ea6855…` | `1f1652bb…` | |

Radio, objetivo y centro de ajuste coinciden. Solo yaw y pitch difieren, y la cámara de ON es exactamente la inicial. Una repetición en OFF dio yaw 5,5223 / pitch 0,9076 (digest `92f4e6a0…`): el reposo de OFF no es siquiera reproducible, lo que descarta un sesgo determinista del builder. `setCamera` devolvió `true` en ambos, y la cámara se mantuvo estable durante todo el sondeo: el desvío ocurrió *antes* de que empezara a sondear.

### Mecanismo

El host (`main.js`) acumula los deltas de ratón en `cameraDelta` y solo los envía con el siguiente mensaje `frame`, y como máximo uno en vuelo (`framePending`). Con OFF el worker pasa cientos de ms/s en construcciones síncronas, así que parte de la órbita sigue en el host cuando termina el gesto. La sonda enviaba `setCamera` (mensaje `api`, inmediato) y el siguiente `frame` aplicaba después el resto de la órbita, dejando la cámara en otra pose. Con ON los frames se responden cada ~16 ms y el atraso se vacía durante la órbita.

### Descartes

| Hipótesis | Resultado |
| --- | --- |
| H1: escena de una época/cámara intermedia en ON | No. La cámara de ON en reposo es la pedida; `lastSentView` coincide con ella (época 206 aplicada, 1246 vistas enviadas). |
| H2: parámetros de construcción distintos | No. Límites, viewport, `targetPx` y escala idénticos. |
| H3: caché/corte distinto | No como causa; el corte difiere solo porque la cámara difiere. Mismo `cacheBytes` (256 MiB) en ambos. |
| H4: color | No. Ambos `rgb`, sin rango de atributo. |

### Experimento de confirmación

Cambio mínimo en la sonda: antes de `setCamera`, esperar 2 respuestas de frame nuevas (vaciar el atraso), y exigir que `getCamera` coincida con la pose pedida (`restTrace.poseRestored`) antes de aceptar el digest. Resultado en Majadas 4 px, Chrome con adaptador de hardware:

| Brazo | Cámara en reposo (yaw / pitch) | `sceneDigestAtRest` |
| --- | --- | --- |
| OFF | 0 / 0,78540 | `1f1652bb0a8161ec` |
| ON | 0 / 0,78540 | `1f1652bb0a8161ec` |

El digest de OFF pasa a ser el de ON. Los dos caminos construyen bytes idénticos para la misma cámara, también con el dataset grande.

## Arreglo

Aplicado (solo sonda y volcado de depuración, sin cambio de comportamiento del visor): vaciado de frames + verificación de pose en `sceneDigestAtRest`; `sceneInputs` en el worker de render. Commit `test(web-host): dump the scene inputs at rest and restore the probe pose reliably`.

Opciones para el defecto de fondo (entrada de cámara en el host retenida tras un `setCamera`), **solo propuestas**:

| Opción | Ventaja | Coste |
| --- | --- | --- |
| A. Que `apiCall("setCamera")` descarte `cameraDelta` pendiente en `main.js` | Corrige la semántica para cualquier cliente de la API | Cambia el comportamiento del host; un usuario que arrastra mientras un script fija la cámara pierde ese arrastre |
| B. Dejar solo el arreglo de la sonda (aplicado) | Cero riesgo en el visor | Otros scripts que mezclen gestos y `setCamera` deben vaciar frames igual |

Recomendación: B ahora; A solo si aparece un cliente real de la API afectado.

## Límites

- Los digests anteriores `34ea6855…`/`92f4e6a0…` corresponden a cámaras distintas de la ajustada y no deben compararse con ON.
- No se volcaron ids de nodos del corte ni bloques residentes: la divergencia se cerró en la cámara, antes de esos campos.
- Una sola ejecución por brazo tras el arreglo (la comparación es bit a bit, no estadística).
