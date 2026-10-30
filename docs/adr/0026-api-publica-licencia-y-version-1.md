# 0026 API pública primero, licencia MIT OR Apache-2.0 y publicación solo en la 1.0

Estado: Aceptada · Fecha: 30-10-2026

## Contexto
Los usuarios son integradores (librería embebible) y usuarios de una app web pública. Hoy no hay API pública estable ni licencia declarada.

## Decisión
- **API primero:** un paquete npm en TypeScript tipado es la única superficie para todas las funciones: operaciones y estado, eventos, consultas de datos y capas de render propias (RF-11 a RF-15). La app de demostración usa solo esa API.
- Los crates de Rust son internos; publicarlos más adelante no se descarta.
- No se publica nada (npm ni app pública) hasta la **1.0**: RQ-11 superado, V7 final congelado y API que cubra RF-10 a RF-15.
- Licencia **MIT OR Apache-2.0**.
- UI y mensajes de la API, solo en inglés.
- Dónde se aloja: sin decidir (OQ-01).

## Alternativas
| Opción | Descartada porque |
| --- | --- |
| npm más crates públicos | Dos superficies de compatibilidad que mantener; publicar los crates queda abierto para más adelante. |
| Solo crates | Cada integrador tendría que compilar su propio wasm; no habría paquete JS oficial. |
| Publicar 0.x con SemVer | El propietario prefiere no publicar nada hasta tener una API estable. |
| 1.0 en la primera publicación | La 1.0 se define por RQ-11, V7 final y API completa, no por la primera publicación. |
| MPL-2.0, AGPL-3.0, privada | MIT OR Apache-2.0 es el estándar de Rust: máxima adopción como librería y compatible con las dependencias (`lz4_flex` es MIT). |

## Consecuencias
- **Pendiente:** archivos `LICENSE-MIT` y `LICENSE-APACHE` y campo `license` en `Cargo.toml`.
- El `createViewer` actual es la semilla de la API; la UI de demostración pasa a inglés.
- Controles de la app pública con WCAG 2.2 AA (RQ-10), verificados con axe y Playwright.
- Cualquier función nueva del visor nace en la API, no en la demo.

## Revisar si
Aparece un integrador que necesite los crates de Rust, o OQ-01 impone restricciones de distribución.

## Evidencia
[Requisitos RF-10 a RF-15, CON-03 a CON-06, RQ-10, RQ-11, OQ-01](../requirements.md)
