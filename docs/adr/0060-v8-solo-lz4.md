# 0060 La V8 comprime solo con LZ4: sin zstd en `.octattrs`

Estado: Aceptada · Fecha: 21-11-2026 · Decide: propietario · Cierra D3 del [ADR 0058](0058-formato-v8-experimental.md) y del [diseño de la V8](../records/archive/formato-v8.md#resoluciones-tras-medir-v81-pre-propietario-20-11-2026)

## Contexto
- D3 quedó abierta como experimento: `v8.2-pre` midió cuatro esquemas fijos (`lz4`, `cold`, `cold-rgb`, `all`)
  ([plan](../records/archive/formato-v8-pre2.md)).
- Medido el 20-11-2026 ([registro](../records/benchmarks/FORMATO_V8_PRE2_20-11-2026.md)): frente a `lz4`, `all` reduce
  la salida un −9,45 % en Albuera, −8,73 % en Majadas y −7,86 % en Belvedere. Convierte como mucho ~3 % más lento, y en
  el navegador la diferencia de carga y decodificación no es medible. `cold` y `cold-rgb` ahorran menos sin ventaja
  medida.
- zstd en wasm exige una dependencia en C (`zstd-sys`) y clang con destino wasm32 para compilar el wasm
  ([tarea 0 del plan](../records/archive/formato-v8-pre2.md#codificador)). Además añade
  códecs, compuertas por layout, casos de paridad y tests que mantener.
- V2 (Albuera ≥ 35 % más pequeña que v7) ya se cumple sin zstd: v8.1-pre da −40,6 %
  ([registro de v8.1-pre](../records/benchmarks/FORMATO_V8_PRE1_20-11-2026.md)).

## Decisión
El propietario (21-11-2026), tras medir v8.2-pre: **la V8 congelada usa solo LZ4**, como v8.1-pre. No lleva códec zstd ni
la opción `--attr-codec`.

## Alternativas
| Opción | Por qué no |
| --- | --- |
| `all` (zstd en todo `.octattrs`) | −7,9 a −9,5 % de salida, con un coste de conversión del orden del ruido y sin diferencia medible en el navegador, pero con una dependencia C permanente y más superficie que mantener; V2 ya se cumple |
| `cold-rgb` o `cold` | Mismo coste de dependencia que `all`, con menos ahorro (`cold`: 0 % en dos de tres nubes) |

## Consecuencias
- La V8 se congela sobre el layout de v8.1-pre (D4). Al congelar se retiran el códec zstd, la dependencia `zstd`,
  `--attr-codec` / `attrCodec` y sus casos de paridad. `v8.2-pre` queda como revisión experimental documentada y no se
  mantiene (ADR 0058, D6).
- La salida de Albuera queda en 9,36 GB (v8.1-pre) en lugar de ~8,48 GB.

## Revisar si
El disco de la salida vuelve a ser el límite (p. ej. nubes mucho mayores que Albuera), o aparece un decodificador zstd en
Rust puro (o nativo del navegador) con velocidad en wasm comparable que elimine la dependencia en C.
