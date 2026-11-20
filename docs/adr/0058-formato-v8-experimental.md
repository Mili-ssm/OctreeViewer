# 0058 Se abre el formato V8 como revisiones previas experimentales

Estado: Aceptada · Fecha: 20-11-2026 · Decide: propietario · Precisa el [ADR 0047](0047-v5-heredado-hasta-v8.md) (empezar V8 retira V5)

## Contexto
- [formato-v8](../plans/formato-v8.md) reúne las decisiones del propietario del 20-11-2026 para una revisión nueva del formato.
- v7 está congelado ([ADR 0044](0044-v7-final-congelada-y-predeterminada.md)); los cambios de carga útil necesitan una identidad nueva.
- La [medida de tamaño de v7](../records/benchmarks/TAMANO_SALIDA_V7_20-11-2026.md) lista los candidatos para reducir la salida.

## Decisión
Decisiones del propietario (20-11-2026):

1. **D1**: se abre una revisión nueva del formato, V8, con su ADR.
2. **D2**: los puntos LOD guardan solo lo que dibuja el visor (coordenadas, RGB, intensidad y clase); las hojas guardan todos los atributos.
3. **D3**: zstd solo si una medida (tamaño, tiempo de conversión y lectura en wasm) lo justifica; la decide el propietario con las cifras.
4. **D4**: el visor y el conversor soportan solo V8; con el ADR 0047 empezar V8 retira V5, y D4 retira además v6 y v7 y sus revisiones previas, cuyos ficheros se reconvierten.
5. **D5**: V8 guarda una estimación estocástica de la distancia mínima entre puntos (cuantiles de la distancia al vecino más cercano), como tamaño mínimo de vóxel recomendado. Que la demo la use en lugar de los 0,1 mm por defecto se decide más adelante: hoy lo fija la resolución de OQ-13 del 18-11-2026, RQ-09 y el [ADR 0054](0054-el-objetivo-de-px-es-el-tamano-minimo.md).
6. **D6**: V8 está en desarrollo y en pruebas: no se fija nada todavía. Se trabaja en revisiones previas (`v8.1-pre`, `v8.2-pre`…), como v7, y se congela cuando las pruebas lo justifiquen.

Decisión técnica del diseño (expuesta al propietario, sin objeción; no es D2): el color de pantalla no se guarda dos veces, se deriva de los datos guardados al leer.

## Alternativas
Candidatos de la medida de tamaño y de [formato-v8](../plans/formato-v8.md) que no se adoptan:

| Opción | Descartada o aplazada porque |
| --- | --- |
| LOD más ralo (×4 o ×8 por nivel; −32 % / −43 %) | Revierte el [ADR 0020](0020-piramide-lod-densa.md) y da saltos de nivel más bruscos; fuera de alcance |
| LOD como referencias a filas de hoja (hasta −49 %) | Rompe la lectura independiente por nodo; fuera de alcance |
| zstd en todo | Medido en Albuera: −16 % de salida, pero `ruzstd` descomprime a ~250 MB/s frente a 3,6–4,9 GB/s de LZ4 en wasm; aplazado a las medidas (D3) |
| Opción del conversor para omitir atributos (−23 %) | Con pérdida y a elección del usuario; no es el camino de V8 |

## Consecuencias
- Hasta el final de V8 conviven v7 y `v8.N-pre`. Por el plan de implementación ([formato-v8-pre1](../plans/formato-v8-pre1.md), restricciones globales), v7 sigue siendo la salida predeterminada y `v8.1-pre` solo se escribe si se pide; v7 no cambia.
- `scripts/check_version_literals.sh` mantiene los literales de versión dentro de `viewer_format`.
- La retirada de V5, v6 y v7 (D4) llega con una fase propia y no cambia ahora ningún lector.

## Revisar si
Las medidas de las revisiones previas no alcanzan los objetivos del plan, o el propietario decide congelar V8.
