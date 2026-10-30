# 0017 Zonas OPFS paginadas para los spools del conversor web

Estado: Aceptada · Fecha: 30-10-2026

## Contexto
- `convert_serial` es una sola llamada wasm síncrona y no hay `SharedArrayBuffer` (R8): dentro de la
  llamada no se puede esperar a `createSyncAccessHandle`, que es asíncrono. Los handles han de abrirse
  antes.
- Un fichero OPFS por spool (decisión 2) cuesta 22–40 ms por fichero en Firefox ([S9](../records/benchmarks/OPFS_SPOOL_FILES_2026-10-29.md)).
  Belvedere necesita 542–1.246 handles y Majadas ~6.000; S9 solo probó 664.
- M3a guardaba los spools en la memoria wasm (`MemSpoolStore`). El pico modelado de Belvedere con
  divisiones 2 (4,35 GiB, [S10](../records/benchmarks/HYBRID_SPOOL_RAM_2026-10-29.md)) supera el techo de 4 GiB de wasm32.
- La decisión 3 (P3 literal: solo el coordinador vuelca) exigía trabajos por pasos, esfuerzo L, y el
  mismo coste por fichero.

## Decisión
Decisión 5 del propietario (30-10-2026):
- **Segmentos de JS** fuera de wasm: 32 KiB que duplican hasta 1 MiB, recortados al cerrar,
  reutilizados por clases de tamaño. Los bytes cruzan por un import tipado (`&[u8]`, vista por llamada).
- **Volcado síncrono** por encima del presupuesto a **páginas de 256 KiB** de **zonas OPFS de 1 GiB**,
  abiertas antes de la llamada. Páginas libres reutilizadas por el índice más bajo; las zonas vacías se
  truncan a mitad de conversión.
- **Lector anclado:** nunca se vuelca un spool con lector vivo.
- **Orden P3 con pistas del planificador** (`hint_retained`, `hint_consume_rank`): cerrados `retained`,
  cerrados por rango, cerrados sin pista, abiertos; presupuesto blando.
- **Zonas = ⌈2 · volcado estimado / zona⌉ + 1**, anunciadas como parciales para su limpieza.
- **Chrome `file://`** (sin OPFS): solo RAM, con rechazo temprano si spools + salida superan `ram_budget`.
- **Presupuesto de spools por defecto: 2 GiB** (decisión 3, resuelta con los datos de M3b).

## Alternativas
| Opción | Descartada porque |
| --- | --- |
| P3 literal con trabajos por pasos | Esfuerzo L ahora y Firefox sigue pagando 22–40 ms por fichero. |
| Pool de handles, uno por spool | 542–1.246 handles abiertos en Belvedere y ~6.000 en Majadas; solo hay evidencia hasta 664. |
| Páginas de 1 MiB | 1,98 × el pico vivo volcado frente a ≤ 1,18 × con 256 KiB ([política](../records/benchmarks/HYBRID_POLICY_2026-10-30.md)). |
| Presupuesto de 4 GiB | Mismo tiempo dentro de la dispersión y 2,3 GiB más de PSS ([M3b](../records/benchmarks/M3B_BELVEDERE_2026-10-30.md)). |

## Consecuencias
- Enmienda las decisiones 2 y 3 de la [spec](../plans/conversor-web-paralelo.md); el SHA no cambia
  porque el planificador y el orden de commit son los mismos (R1).
- Resultados ([Node](../records/benchmarks/M3B_NODE_GATE_2026-10-30.md), [navegadores](../records/benchmarks/M3B_BELVEDERE_2026-10-30.md)):
  Belvedere con SHA = R1 en Chrome y Firefox, 2,1–2,9 × `native_hybrid`, PSS 3,4–4,5 GiB; en Node 2,8 ×.
  Memoria wasm 28–40 MiB.
- Coste: el núcleo gana un almacén con política (`pipeline/hybrid/`) y el host un módulo de tiers y
  zonas; la cuota OPFS debe cubrir las zonas anunciadas.
- Con varios workers (M5) los spools pasan por el líder, que hará de conductor de un plan por pasos;
  pistas y modelo suponen recorrido en profundidad con N = 1.
- Chrome libera cuota solo al cerrar el handle y tarda ~2 s en soltarlo tras `terminate()`: la limpieza
  reintenta cada 50 ms hasta 10 s.

## Revisar si
- Firefox abre ficheros OPFS en < 3 ms (un fichero por spool volvería a ser viable).
- COOP/COEP o `SharedArrayBuffer` pasan a estar disponibles (R8).
- El máximo de páginas en uso supera 1,3 × la estimación de zonas en cualquier conversión medida.

## Evidencia
[Zonas OPFS](../records/benchmarks/OPFS_ZONES_2026-10-30.md) ·
[Política](../records/benchmarks/HYBRID_POLICY_2026-10-30.md) ·
[Etapa 4](../plans/conversor-web-paralelo-etapa4.md) ·
[ADR 0014](0014-nucleo-de-conversion-sin-io.md) · [ADR 0015](0015-pool-de-workers-sin-memoria-compartida.md)
