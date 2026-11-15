# Exploración: velocidad de conversión v7.5 frente a V5 (fase C) — 14-11-2026

**Estado: tres optimizaciones con salida idéntica y sin ganancia suficiente; salta el criterio de
parada; RQ-07 pendiente del propietario.** RQ-07 pide v7.5/V5 ≤ 1,3; en Belvedere sigue en ~1,9. El
perfil sitúa la diferencia en la construcción de la jerarquía (muestreo), y ninguna de las tres
optimizaciones la reduce de forma relevante. Nada de esto se integra.

## Línea base (C0)

Belvedere, perfil `perf`, 3 conversiones intercaladas por formato, tiempo de pared:

| Formato | Total (mediana; mín.–máx.) | SHA-256 `.oct` |
| --- | --- | --- |
| V5 | 20,9 s (20,5–21,4) | `ecdc4889…` |
| v7.5 | 40,2 s (37,9–41,0) | `101464e9…` |

Razón v7.5/V5: **1,92**. Equipo con 20 núcleos, gobernador `powersave` y carga de 2 a 6 por otras
sesiones activas: el ruido de fondo ronda el 2–3 %.

## Dónde está la diferencia

Mediana de 6 ejecuciones por etapa (límites de etapa según lo que imprime el conversor):

| Etapa | V5 | v7.5 | Diferencia |
| --- | --- | --- | --- |
| Planificar el índice (decodificar LAZ) | 9,7 s | 9,9 s | +0,2 s |
| **Construir la jerarquía** | 5,4 s | 17,8 s | **+12,4 s** |
| Leer y escribir bloques | 3,9 s | 9,2 s | +5,3 s |
| Validar la salida | 1,9 s | 3,0 s | +1,1 s |

Muestreo de pilas con `eu-stack` (`perf` no está instalado; porcentajes aproximados): en la jerarquía
de v7.5, `SampleAccumulator::add`, `SampleAccumulator::nearest` y `GlobalCellFrame::cell` suman ~37 %
de las muestras activas y apenas aparecen en V5. En los bloques, la diferencia está en los
codificadores (LZ4 y atributos), no en la aritmética `i128`. Las dos rutas usan ~3,6–3,9 hilos
activos, así que el paralelismo (TC7) no explica la razón.

## Lo que se probó

Las tres con salida SHA-256 idéntica en V5 y v7.5 (`.oct` y `.octattrs`), en ramas `explore/`:

| Propuesta ([revisión](REVIEW_TECHNIQUES_13-11-2026.md#conversor)) | Rama y commit | Efecto |
| --- | --- | --- |
| TC3: `i128` fuera del bucle por punto | `explore/convert-speed`, `9adb933` | Bloques −1 % (dentro del ruido); total igual |
| TC1: celda y distancia desde los enteros | `explore/convert-speed`, `36e10f1` | Sin cambio; el punto caliente sigue igual |
| TC2(a): datos calientes separados | — | No se probó: las sumas de color se escriben en cada punto, así que no son frías |
| TC2(b): lotes ordenados por celda (radix estable) | `explore/convert-batch`, `6efc4f0` | Jerarquía v7.5 −8 % (17,8 → 16,4 s); razón 1,86. No llega al −30 % exigido. El tamaño de lote (64 KiB–8 MiB) no influye |

Con TC2(b), el coste se traslada a la reducción por tramos, sin bajar. El límite parece el trabajo
por punto (sumas de color, comparación de distancia y copia del registro completo cada vez que
aparece un punto más cercano), no el acceso aleatorio a las celdas (*inferido*). No se midió quitar la
copia del registro ni diferir la copia del más cercano.

## Siguientes pasos posibles (decisión del propietario)

- **TC5:** rediseño estructural del pipeline (distribución en dos pasadas y construcción por *chunk* en
  memoria, como PotreeConverter 2). Necesita plan y pre-mortem propios.
- **Revisar RQ-07:** v7.5 construye una pirámide densa (ADR 0020) y hace más trabajo por diseño;
  fijar otro objetivo o medirlo de otra forma.
- **Copia diferida del más cercano:** guardar solo el índice del punto más cercano y copiar el registro
  al final de la celda. Es un cambio acotado que no se ha probado.

## Identidad y reproducción

- Base: `feat/r3-v75` en `8213e66`. Perfil `perf` (`opt-level` 3, LTO fino). Entrada:
  `belv_2009_digital-aerial_pcd_copc.laz` (fuera del repositorio, `data/datasets/`).
- Conversión: `viewer_convert <entrada.laz> <salida.oct> --format-version v5|v7.5-pre`, con marca de
  tiempo de cada línea de etapa del conversor. Los scripts de medida no están en git.
- Una sola máquina, una sesión; las cifras solo valen para esta revisión y este equipo.
