# 0018 El worker de construcción se activa por defecto

Estado: Aceptada · Fecha: 30-10-2026

## Contexto
- La voxelización en CPU es el 98–99,5 % del tiempo ocupado del worker del visor ([línea base](../records/benchmarks/RENDER_BASELINE_2026-10-30.md), B1 de la [evaluación](../records/archive/RENDER_ASSESSMENT_2026-10-30.md)).
- Sin worker de construcción, el intervalo entre respuestas de frame tiene p95 ≈ 1 s en Majadas y 5–7 s en Belvedere. Con él: p95 ≤ 17,6 ms, máx ≤ 33,9 ms, tiempo hasta el objetivo −57–62 %, misma cobertura y evicciones, PSS +40–60 MiB ([medición](../records/benchmarks/RENDER_BUILD_WORKER_2026-10-30.md)).
- El digest en reposo es idéntico con y sin worker; la diferencia vista en Majadas era del arnés ([diagnóstico](../records/benchmarks/RENDER_BUILD_WORKER_DIGEST_2026-10-30.md)).
- Si el navegador no permite workers anidados (Chrome `file://`), el visor cae a la ruta dentro del worker.
- La [ADR 0015](0015-pool-de-workers-sin-memoria-compartida.md) dejó la lectura del visor a una decisión propia: esta.

## Decisión
- El worker de construcción queda **activado por defecto** desde la fusión de `feat/web-converter-stage4`.
- Con la cámara en movimiento se reconstruye con la **última vista** y se sigue dibujando la escena anterior (cubos en coordenadas de mundo) hasta que llegue la nueva.
- Se mantiene el respaldo a la ruta dentro del worker.
- Su papel se reevalúa al llegar R3 ([ADR 0019](0019-cubos-por-nodo-residentes-en-gpu.md)); probablemente quede como worker de datos (lector, caché, planificador).

## Alternativas
| Opción | Descartada porque |
| --- | --- |
| Opcional hasta R3 | Deja la latencia de frames fuera de RQ-01 en la ruta por defecto sin ganancia. |
| Descartarlo e ir directo a R3 | Sin plan ni código de R3, deja la ruta por defecto sin la mejora ya medida; el worker se reutiliza como worker de datos. |
| Reconstruir solo en reposo | La escena quedaría congelada durante todo el movimiento. |
| Reconstruir solo si cambia el corte | Motivo no registrado por el propietario; la última vista ya fusiona reconstrucciones. |

## Consecuencias
- La escena llega desfasada entre B y 2B (B = tiempo de construcción: 0,48 s en Majadas, 2,6 s en Belvedere a 8 px).
- R1-T2 (llegadas agrupadas) queda cancelada: las vistas ya se fusionan en el worker.
- **Pendiente:** cambiar el valor por defecto (hoy `buildWorker` está apagado) y actualizar la documentación que lo describe como opcional.
- +40–60 MiB de PSS; el wasm de construcción ocupa 123–204 MiB.

## Revisar si
- R3 sustituye la voxelización CPU y el worker deja de aportar frente a su coste de memoria.
- Una medición en Firefox o en otra GPU incumple RQ-01 con el worker activo.

## Evidencia
[Preguntas P7 y P8](../explanation/preguntas-propietario.md) · [Plan de render, R2](../explanation/render-y-carga-escala.md) · [Requisitos RF-08](../requirements.md)
