# 0010 Migrar el renderer a wgpu/naga 30 y glam 0.34

Estado: Aceptada · Fecha: 14-10-2026 (según el documento de migración)

## Contexto
Mantener wgpu antiguo acumula deuda; la migración cambia contratos de superficie/readback.

## Decisión
El workspace usa wgpu 30 y glam 0.34; compila en nativo/WASM y pasa checks nativos y readback por software. La validación de navegador es parcial (pérdida de dispositivo reproducida también con un clear independiente).

## Alternativas consideradas
| Opción | Descartada porque |
| --- | --- |
| Permanecer en la versión anterior | Deuda creciente y APIs obsoletas. |

## Consecuencias
Validación de navegador pendiente; readback/SwiftShader no es prueba visual.

## Revisar si
Se reproduce y corrige la pérdida de dispositivo, o wgpu publica cambios incompatibles.

## Evidencia
[RENDERER_DEPENDENCIES](../explanation/RENDERER_DEPENDENCIES.md)
