# 0058 Se abre el formato V8 como revisiones previas experimentales

Estado: Aceptada · Fecha: 20-11-2026 · Decide: propietario · Precisa el [ADR 0047](0047-v5-heredado-hasta-v8.md) (empezar V8 retira V5)

## Contexto
- [formato-v8](../plans/formato-v8.md) reúne las decisiones del propietario del 20-11-2026 para una revisión nueva del formato.
- v7 está congelado ([ADR 0044](0044-v7-final-congelada-y-predeterminada.md)); los cambios de carga útil necesitan una identidad nueva.

## Decisión
1. **D1**: se abre una revisión nueva del formato, V8.
2. **D6**: V8 está en desarrollo y en pruebas. Se trabaja en revisiones previas (`v8.1-pre`, `v8.2-pre`…), como v7, y se congela cuando las pruebas lo justifiquen. La salida predeterminada sigue siendo v7.
3. **D2**: los puntos LOD guardan solo lo que dibuja el visor (coordenadas, RGB, intensidad y clase); las hojas guardan todos los atributos.
4. El color de dibujo no se duplica: se deriva de los datos guardados.
5. **D3**: zstd solo entra si una medida (tamaño, tiempo de conversión y lectura en wasm) lo justifica; la decide el propietario con las cifras.
6. **D4**: al final el visor y el conversor soportan solo V8; V5, v6 y v7 (y sus revisiones previas) se retiran y sus ficheros se reconvierten.
7. **D5**: V8 guarda cuantiles de la distancia al vecino más cercano como estimación del tamaño mínimo de vóxel recomendado.

## Alternativas
| Opción | Descartada porque |
| --- | --- |
| Cambiar v7 en su sitio | v7 está congelado; rompería los ficheros existentes |
| Fijar V8 desde la primera revisión | Faltan medidas que lo justifiquen (D3, D6) |

## Consecuencias
- Hasta el final de V8 conviven v7 (predeterminado) y `v8.N-pre` (opt-in); `scripts/check_version_literals.sh` mantiene los literales de versión dentro de `viewer_format`.
- La retirada de V5/v6/v7 (D4) llega con una fase propia y no cambia ahora ningún lector.

## Revisar si
Las medidas de las revisiones previas no alcanzan los objetivos del plan, o el propietario decide congelar V8.
