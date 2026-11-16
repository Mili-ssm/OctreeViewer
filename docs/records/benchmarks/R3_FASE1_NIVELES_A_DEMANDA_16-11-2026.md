# R3 fase 1: niveles a demanda (#35) — 16-11-2026

**Resultado:** el corte pide cualquier profundidad del rango construible de cada nodo, y el worker de construcción
construye a demanda los niveles que la cadena podada no guarda. La nube de #35 deja de verse como puntos sueltos.
Plan: [fase-1-niveles-a-demanda.md](../../plans/fase-1-niveles-a-demanda.md); decisión:
[ADR 0048](../../adr/0048-el-render-dibuja-el-nivel-pedido.md).

**Identidad:** rama `feat/on-demand-levels` (base `f15381d`, 18 commits hasta `9bbdf6c`), perfil `fast` para los
tests, Chromium 1247 sin GPU (SwiftShader) para el navegador, equipo de desarrollo del propietario.

## Hechos

| Comprobación | Resultado |
| --- | --- |
| `scripts/check.sh` con `FORMAT_PARITY_REAL_INPUT` (Belvedere) | 24/24 pasos |
| Tests de librería | `viewer_render_wgpu` 317, `viewer_wasm` 150, JS 238 |
| Escenario #35 en simulación (`a_close_pair_leaf_draws_at_its_spacing_from_far`) | Hoja de 100 pares cercanos vista de lejos a 4 px: cadena completa 3–13, podada [12, 13]; el oráculo pide 6 y se dibuja 6 en las 8 octantes, construido a demanda |
| Control negativo sin construcción a demanda | Se dibuja 12 (la profundidad fina guardada) frente a 6: el fallo de #35 |
| Puerta de engrosamientos en las simulaciones de streaming | 0 sin justificar en las 18; solo se excusan las etapas contadas de ese nodo (R3-Q11) o de la escalera por encima del objetivo |
| Navegador, `fs_e430_n4008_18.las` convertido a V7 | Franja continua (antes unos 25 puntos sueltos, [#35](https://github.com/Mili-ssm/OctreeViewer/issues/35)); objetivo alcanzado en 5,7 s (escritorio, en frío) y 1,2 s (móvil) |
| Navegador, muestra integrada | Objetivo alcanzado en escritorio y móvil; «Recreate viewer» la reabre; consola sin errores |
| Navegador, zoom sobre los datos | El estado vuelve a «target reached» y se mantiene (unos 25 s con SwiftShader) |

## Fallos anteriores a la fase 1 que salieron y se arreglaron

- **Relevo padre → hijo más grueso (R3-Q21):** un hijo que solo guardaba niveles más gruesos que los que dibujaba
  el padre lo relevaba de todas formas (13.898 eventos en las simulaciones). Lo ocultaba el recorte del nivel deseado a
  lo guardado. La retención mide ahora contra el nivel deseado real.
- **Bajada al residente grueso mientras sube el pedido (R3-Q21):** con el nivel pedido guardado pero aún subiéndose,
  se dibujaba el residente más grueso aunque el más fino seguía en GPU. Ahora se dibuja el residente más fino.
- **Ajuste al pool sin salida:** con empates de tamaño entre niveles de una página, el ajuste no bajaba nada y el
  desbordamiento no terminaba.

## Inferencias y pendientes

- **Pendiente:** el p95 de petición a residente y la longitud de la cola durante un zoom de 10 s (criterio de parada
  de D1) no se midieron: el visor no expone esos contadores y SwiftShader no da tiempos representativos. Se miden en la
  fase 2 con contadores y en GPU real.
- **Inferido:** la memoria en reposo crece en las simulaciones (206 → 569 KB en un caso) porque ahora se dibujan los
  niveles pedidos; con datos reales la acota la regla de residencia D4 (fase 2b).
- El estado «refining» con los datos fuera de vista ya ocurría antes ([#42](https://github.com/Mili-ssm/OctreeViewer/issues/42)).
