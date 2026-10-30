# 0024 Hilos wasm opcionales cuando la página está aislada (COOP/COEP)

Estado: Aceptada · Fecha: 30-10-2026 · Enmienda a [0015](0015-pool-de-workers-sin-memoria-compartida.md)

## Contexto
La [ADR 0015](0015-pool-de-workers-sin-memoria-compartida.md) descartó los hilos wasm porque `http.server` y `file://` no sirven COOP/COEP. El propietario no exige esas cabeceras, pero un sitio que las sirva podría aprovechar memoria compartida ([P6](../requirements.md#resoluciones-del-30-10-2026)).

## Decisión
- No se exige COOP/COEP.
- Si la página está aislada (cross-origin isolated), se pueden usar hilos wasm con memoria compartida; si no, el pool de workers de la ADR 0015.

## Alternativas
| Opción | Descartada porque |
| --- | --- |
| Exigir COOP/COEP | Rompe `file://` y el servidor simple. |
| Solo pool sin memoria compartida | Renuncia a la memoria compartida donde el sitio sirva las cabeceras (CON-02). |

## Consecuencias
- Dos compilaciones wasm: los atómicos exigen `build-std` en nightly ([ADR 0011](0011-nightly-obligatorio-con-fecha-fija.md)).
- Doble matriz de pruebas; ambas rutas deben producir los mismos bytes (RQ-08).
- La prioridad la fija el plan que lo use primero. **Pendiente:** nada implementado.

## Revisar si
El pool sin memoria compartida cumple las puertas de conversión con M6, o el coste de mantener dos rutas supera la ganancia medida.

## Evidencia
[Requisitos CON-02, CON-07](../requirements.md) · [Spec del conversor web paralelo](../plans/conversor-web-paralelo.md)
