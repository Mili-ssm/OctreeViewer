# R3 B5: recuperación ante la pérdida del dispositivo WebGPU — 14-11-2026

**Estado: pasa.** Tras una caída provocada del proceso GPU de Chrome, el visor detecta la pérdida, pide
adaptador y dispositivo nuevos, vuelve a subir la escena desde el worker de construcción y sigue
dibujando **sin recargar el dataset ni la página**, con la misma pose y el mismo corte
(R3-Q26). Como el `OffscreenCanvas` transferido deja de presentar tras la caída, la API sustituye el
canvas del host por un clon y lo anuncia con `voxelviewer:canvasReplaced` (R3-Q34).

| Comprobación | Resultado |
| --- | --- |
| Pérdida detectada | sí, motivo `unknown` («A valid external Instance reference no longer exists.») |
| Recuperado | al primer intento, 0,61 s después de la caída |
| Sin recarga | sí (mismo marcador de carga de la página) |
| Misma pose | sí |
| Mismo corte | sí, `cutDigestAtRest` `f2bbcfeaaff029c0` antes y después |
| Dispositivo nuevo sano | sí (`deviceLost` nulo) |
| Lienzo pintado | sí, en el canvas sustituido |
| Errores de página | 0 (en la consola, el 404 sin URL de siempre) |

## Cómo funciona

- El renderer registra el callback de pérdida de dispositivo de wgpu y la expone en
  `gpuStats().deviceLost {reason, message}`. Un motivo `destroyed` no se recupera, y entonces se deja de vigilar.
- El worker de render vigila la pérdida y recrea el visor: hasta 3 intentos, con esperas de 100, 500 y
  2.000 ms. Vuelve a aplicar tamaño, color, sombreado, continuidad y modo de px, reabre el dataset sin
  releer el fichero y avisa al worker de construcción (`driverReplaced`), que reenvía las celdas.
  Después restaura la pose. Si la pérdida llega mientras se abre un dataset, se recupera ese dataset,
  nunca la muestra de demo. Si fallan los 3 intentos, emite un único error.
- La API (`viewer-api.mjs`) clona el canvas del host (id, clases, atributos, estilo y tamaño), lo
  sustituye en el DOM, mueve el foco si lo tenía, vuelve a enlazar sus propios listeners y transfiere el
  canvas nuevo al worker fuera de la cola de mensajes. El worker lo espera hasta 2 s y, si llega tarde,
  se recrea sobre él. Los listeners que el host añadió al canvas anterior no se copian: el host debe
  volver a enlazarlos en `voxelviewer:canvasReplaced`, como documenta `examples/web-host/README.md`.

## Límites

- Una sola caída, provocada con el comando CDP `Browser.crashGpuProcess`, en un equipo (Radeon 880M,
  Chrome 157 sin interfaz). Navegar a `chrome://gpucrash` desde otra pestaña no provocó ninguna pérdida
  en 15 s.
- La ruta CPU sin worker de construcción solo la cubren tests de Node, no el navegador.
- Un host que ofrece canvas nuevo pero no responde retrasa la primera recuperación hasta 2 s.
- Las dos guardas principales (dataset en apertura y vigilancia tras `destroyed`) se comprobaron con
  mutaciones: al quitar cada una, falla su test.

## Identidad y reproducción

- Código: rama `explore/b5-device-lost`, revisión `d54ed92`, integrada después en `feat/r3-v75`.
  Entorno: el de [T11](data/r3-t11-smoke-13-11-2026/identity.txt).
- Probe `examples/web-host/probe-device-loss.mjs` con el servidor de
  [T11](data/r3-t11-smoke-13-11-2026/command.sh); datos en
  [data/r3-b5-device-lost-14-11-2026](data/r3-b5-device-lost-14-11-2026/).
