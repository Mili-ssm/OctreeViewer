# Evaluación integral del código — 24-10-2026

Revisión inicial de `master` en `2b6f3a0`, con cambios aún sin confirmar. Se
inspeccionaron los cinco crates, el host web, los contratos, tests y documentos activos.
Es una auditoría por rutas e invariantes, no una prueba de cada combinación de entrada.
Las secciones 1–4 conservan los hallazgos previos a las correcciones locales de la sección 5.
La [tabla de prioridades del README](../README.md#prioridades-pendientes)
continúa siendo la única hoja de ruta activa; este informe aporta evidencia y criterios de cierre.

«Comprobado» significa inspección directa o ejecución indicada; «documentado» es una afirmación
del proyecto; «inferido» exige reproducción o medida. Tests nativos, mocks del host, readback
GPU y presentación en navegador aportan evidencias distintas.

## 1. Documentación frente a código

El [README](../README.md) distingue objetivo, implementación y pendientes. El
[inventario](PROJECT_INVENTORY.md) es orientación fechada; el
[historial](DEVELOPMENT_HISTORY.md) está archivado. Los benchmarks conservan revisión,
entorno y límites, y no se trataron como medidas actuales. Se comprobaron los enlaces
relativos de README, documentos activos, especificaciones de formato y guía web: existen.

| Componente y contrato | Contraste con código y pruebas | Resultado |
| --- | --- | --- |
| `viewer_core`: jerarquía Replacement/Additive, LOD, caché y transición. | `spatial.rs`, `spatial_lod_priority.rs`, `lod_transition.rs` y tests codifican corte, presupuesto y semánticas. | Alineado; continuidad *visual por región* pendiente. |
| `viewer_format`: v5 predeterminado y pre1–pre4 optativos, identidades exactas, pareja separada, CRC, lectura selectiva. | `layout.rs` despacha solo estas revisiones; `contracts.rs`, `storage_reader.rs`, `reader.rs` y tests cubren el contrato. `read_attribute_row` decodifica la columna completa, como documenta la API. | Alineado; dos frases de `FORMAT_V5.md` solo citaban pre1/pre2 y se corrigieron. |
| `viewer_convert`: LAS/LAZ, EPT local `laszip`, spools/checkpoint LAS, publicación no atómica, pipeline opcional. | `lib.rs`, `spatial.rs`, `ept.rs`, `checkpoint.rs` y tests CLI sustentan las rutas. EPT conserva jerarquía Additive, sin checkpoint EPT. | Alineado en funciones; presupuesto EPT tardío y portabilidad por comprobar. |
| `viewer_render_wgpu`: grilla diádica, resolución regional Replacement, límites de instancias, HZB y picking. | `voxel_grid.rs`, `block_sync.rs`, `gpu.rs` y `voxel_regions.rs` cubren grilla y fallback. | Alineado; presupuesto GPU entre rutas merece aclaración. |
| `viewer_wasm`: Blob síncrono en worker, un bloque por actualización, LOD progresivo, conversión OPFS en dos pasadas. | `browser_io.rs`, `lod_dataset.rs`, `browser_convert.rs` y `bindings.rs` respaldan el flujo. | Alineado; transición y memoria conjunta sin cierre. |
| `examples/web-host`: API embebible, cola, cancelación/reinicio, paquete local y métricas. | `viewer-api.mjs`, `converter-api.mjs`, workers y tests Node cubren lógica con dobles. | Contrato general alineado; fuga de recursos en fallo de inicialización. |

La [migración wgpu 30/glam 0.34](RENDERER_DEPENDENCIES.md) está implementada; se corrigió
«migración pendiente» en el README. El README/inventario aún declaran correctamente las
omisiones de CRS, flags y Extra Bytes, EPT remoto, transporte remoto del visor y v6 final.

## 2. Criterios y requisitos de desarrollo

| Criterio | Evaluación basada en evidencia | Pendiente para cierre |
| --- | --- | --- |
| Corrección y compatibilidad | Identidades exactas, rechazo de revisiones sin backend, CRC, límites de lectura y tests de corrupción. Checks habituales pasan. | Resolver la política escritor/lector y plataformas. |
| KISS, YAGNI y cohesión | Crates separan dominio, formato, conversión, renderer y orquestación; pipeline y v6 son optativos. Tamaño de archivo no prueba mal diseño. | Extraer módulos solo al reducir un cambio repetido o acoplamiento real. |
| DRY y contratos públicos | `FormatVersion`, `StorageLayout` y el lector centralizan reglas; `GpuBudget` limita buffers de instancias de escena/bloques y su pico de actualización. | Si se quisiera un límite total de recursos GPU, definir otro contrato con texturas y sus límites de estimación. |
| Memoria y fallos | Lector acota reservas/decodificación; conversor usa spools; builder y renderer tienen modelos y límites. | Presupuesto EPT antes de grandes metadatos; pico conjunto de caché, builder, escena, WASM, RSS/PSS y GPU. Modelos no son RSS/VRAM. |
| Precisión e identidad | Marco/origen `f64`, cuantización `u32`, residuales exactos según revisión; pruebas de negativos y origen grande en grilla. | Fijar CRS, ejes, unidades, ancla y grilla entre nubes; probar proyección de enteros de 64 bits. |
| Errores y cancelación | Errores propagados, temporales limpiados cuando es posible y workers reiniciables. | Limpiar inicialización web fallida; comprobar OPFS, pérdida de dispositivo y carga fallida en navegador real. |
| Rendimiento | Benchmarks fechados con protocolo y límites; Majadas histórico atribuyó la mayor parte del trabajo medido a reconstrucciones del builder. | Repetir en la revisión y cargas objetivo antes de afirmar mejora; medir CPU, memoria, I/O, calidad y latencia juntos. |
| Verificación | Pasaron `cargo test-fast --offline`, fmt, Clippy nativo/WASM, 28 tests Node y 34 Python antes de estos cambios solo documentales. No hay workflow `.github`. | Automatizar checks repetibles si compensa; GPU/navegador requieren validación separada. |

**Requisitos de producto sin decisión:** plataformas y mínimos RAM/GPU, CRS/flags/Extra Bytes,
varias nubes, licencia/compatibilidad, interoperabilidad y alcance de medición, clasificación,
recorte y exportación.
Son decisiones que cambian contrato y prioridad. El objetivo >500 GB y comparar con Potree
requieren cargas, calidad, hardware y tareas equivalentes; no están demostrados.

**Navegador en esta evaluación:** se sirvió el host por HTTP local y se abrió con Playwright
MCP. Cargó la página; a 390 × 844 CSS px no hubo desbordamiento horizontal. El estado visible
fue `init failed: no GPU adapter supports this surface`; `navigator.gpu` existía, pero
`requestAdapter()` devolvió `null`. La consola registró un 404 de `favicon.ico` y un aviso
del adapter. Por ello, esta sesión no valida presentación, cámara, picking, OPFS,
recuperación ni uso móvil real. El paquete WASM generado local no se verificó byte a byte
contra HEAD. La [evidencia histórica](RENDERER_DEPENDENCIES.md#navegador-resultado-parcial-y-contraejemplo-independiente)
ya separaba readback de presentación.

## 3. Problemas y refactorización

P0/P1 de producto señalan impacto y prioridad, no un bug demostrado en todos los entornos.

| Prioridad; certeza | Evidencia y efecto | Alternativa sencilla y comprobación |
| --- | --- | --- |
| P0; pendiente | `lod_transition.rs`/`lod_dataset.rs` retienen cortes en ciertos estados, pero la [entrega regional](REGIONAL_VOXEL_LOD.md#cierre-pendiente) no conserva representación anterior por región en toda transición ni certifica carga lenta/fallida mixta. | Definir invariante regional y reproducir con multibloque real; corregir estado/commit localizado antes de un acumulador global. |
| P0; observado históricamente | Presentación Chrome/AMD y recuperación GPU abiertas; hasta un clear independiente perdió dispositivo. Aquí no hubo adapter. | Separar caso mínimo de plataforma, renderer y host; validar Chrome/Firefox físicos con captura, consola y picking. |
| P1; reproducido con dobles Node | `createViewer` crea Worker antes de validar `mode`, y registra observer/listener antes de `transferControlToOffscreen` (`viewer-api.mjs:14-42,149-160`). Un modo inválido deja el Worker sin terminar; al simular una transferencia fallida quedan Worker, observer y listener. El test existente usa transferencia que nunca falla. | Validar opciones antes de adquirir recursos y limpiar si transferencia/postMessage fallan. Test de excepción que cuente terminate/disconnect/listeners. |
| P1; orden comprobado, magnitud no medida | EPT lee jerarquía y construye nodos/bloques/`SpatialHierarchy` (`ept.rs:171-313`) antes de comprobar `max_memory_budget_bytes` (`324-337`). Límite bajo no evita ese trabajo. Hay tope de 1 M nodos y 64 MiB por página, pero no preflight EPT del presupuesto. | Presupuestar incrementalmente al leer páginas/conteos y rechazar antes de duplicar estructuras. Test EPT con límite bajo y pico instrumentado; sigue siendo estimación, no RSS estricto. |
| P1; pendiente de medida | Caché, builder, escenas previa/candidata, WASM y GPU viven juntos; informes históricos no fijan pico actual. | Instrumentar recorrido completo, comparar cortes equivalentes y reducir reconstrucciones/copias identificadas antes de caché persistente. |
| P3; criterio aclarado | `GpuBudget` promete limitar buffers de instancias; `sync_visible_blocks` cumple ese alcance. `upload_scene` cuenta además HZB/readback y puede rechazar de forma más conservadora (`gpu.rs:459-463,543-552,600-615`). No es un incumplimiento del presupuesto publicado ni un techo de VRAM total. | No cambiar esta ruta ahora. Si un caso medido exige unificar los rechazos, decidir explícitamente entre presupuesto de instancias y otro contrato de recursos GPU, incluidos texturas y límites de estimación. |
| P2; compatibilidad condicionada | Escritor público admite esquemas mayores que los 256 atributos del lector predeterminado (`writer.rs:907`, `reader.rs:22,85-95,767-772`). El archivo puede exigir límites de lector personalizados. | Decidir garantía de apertura predeterminada; validar escritura o documentar lector personalizado, y probarla. |
| P2; plataforma por decidir | Publicación llama `File::open(parent)?.sync_all()?` sin condición (`viewer_convert/src/lib.rs:1388,1405`); checkpoint lo limita a Unix (`checkpoint.rs:493-505`). No hay prueba Windows. | Si Windows es objetivo, definir durabilidad y probar publicación/fallo/limpieza allí. |
| P2; inferido, no medido | API WASM `appendPoints`/`appendVoxelizedPoints` refresca toda la escena por lote (`controller.rs:106-112,142-153,184-191`). Muchos lotes pequeños podrían dar trabajo acumulado cuadrático. | Confirmar consumidores y medir patrón. Solo entonces ofrecer batch/refresco diferido con semántica/API conservadas. |
| P3; inferido, no medido | `resolve_replacement` compara repetidamente objetivos con nodos activos; `voxel_grid.rs` usa `BTreeMap` por determinismo. | Perfilar cortes reales; comparar cambios locales con igualdad de salida, orden y memoria. No sustituir por estructura nueva solo por complejidad teórica. |

Una reestructuración transversal carece de justificación actual. Transición localizada,
adquisición/limpieza simétrica, preflight EPT y presupuestos explícitos son las opciones de
menor coste. Acumulador persistente, revisión de formato, índice global o I/O concurrente
añadirían estado, migración y fallos parciales sin evidencia de que resuelvan el bloqueo.

La reproducción puntual de `createViewer` usó un script Node efímero, sin modificar el
repositorio. `mode: 'invalid'` lanzó con `terminated=0`; una transferencia simulada que lanzó
`already transferred` dejó `terminated=0`, `disconnected=0` y un listener `resize` registrado.
Es evidencia de ciclo de vida con dobles, no prueba de fuga cuantificada en navegador.

## 4. Conclusión y plan

El proyecto tiene separación de responsabilidades, contratos binarios y verificación local
sólidos para una fase experimental. Los bloqueos principales son evidencia visual y escala
integrada; hay además dos problemas locales de ciclo de vida y presupuesto que merecen
reproducción dirigida. No se puede atribuir rendimiento o soporte a un entorno no medido.

| Fase y responsable | Dependencia y entregable | Criterio de cierre |
| --- | --- | --- |
| A. Requisitos; transversal | Acordar plataformas, RAM/GPU, CRS/atributos, licencia/compatibilidad y herramientas; registrar decisión. Avanza en paralelo a B–D. | Matriz de soporte explícita; lo no decidido sigue marcado. |
| B. Calidad local; host/convert | Convertir la reproducción de `createViewer` en regresión automatizada; reproducir presupuesto EPT antes de corregir. Decidir escritor/lector y Windows según A. | Regresiones antes/después, error correcto, recursos liberados, sin output parcial nuevo; checks pertinentes. |
| C. Continuidad LOD; core/render/WASM | Definir regiones/cortes, caso real Replacement/Additive, carga lenta/fallida, movimiento y memoria de escena candidata. | Sin huecos/solapamientos de transición; colores/atributos correctos y escena retenida ante fallo; test nativo y observación visual objetivo. |
| D. Navegador; host/WASM/render | Diagnosticar adapter/presentación con clear mínimo, luego muestra, picking, teclado, escritorio y móvil. Requiere GPU objetivo; independiente de B. | Build identificable, captura y consola por navegador/GPU; recuperación observada, no solo readback. |
| E. Rendimiento integral; transversal | Medir ya conversión, EPT, uso de `append` y CPU de cortes fijos. Tras C/D, completar primera vista/refinamiento, RSS/PSS, WASM, GPU y calidad visual con Belvedere, Majadas y Albuera. | Medianas/dispersión, perfil/versión/entorno y picos simultáneos; decidir con datos batching/caché/selector/codec. |
| F. Evolución condicionada; format/convert | Con A/E, valorar remoto, varias nubes y nuevos codecs; mantener v5 predeterminado y v6 final rechazado hasta decisión explícita. | Compatibilidad, precisión, corrupción, lectura selectiva, WASM y migración documentadas. |

Al cerrar una fase, actualizar la prioridad correspondiente del README y el informe temático,
separando implementación, checks y medidas. Considerar CI para suites repetibles; un pase de
CI no sustituye navegador/GPU.

## 5. Correcciones locales posteriores a la evaluación — 24-10-2026

- **Inicialización del host:** `createViewer` valida el modo antes de crear el Worker y libera
  Worker, observer y listeners si falla la transferencia, el mensaje inicial o el registro de
  recursos. Las regresiones se observaron fallar antes y pasar después; los tests del host
  pasan 33/33. Es verificación con dobles, no una prueba de recuperación WebGPU real.
- **Presupuesto EPT:** el conversor rechaza un presupuesto por debajo del mínimo del modelo
  antes de leer páginas; al leer la jerarquía actualiza una cota inferior por nodos,
  referencias y bloques inevitables. Antes de reservar IDs/estructuras de planificación
  comprueba la estimación con el formato de punto conocido. Dos regresiones cubren el rechazo
  temprano y la ausencia de salidas parciales; los siete tests EPT pasan. El parser aún puede
  usar memoria temporal de una página de hasta 64 MiB: no es un límite estricto de RSS.
- **Contrato GPU:** la revisión de `GpuBudget` confirmó que limita buffers de instancias,
  no toda la VRAM. No se cambió el renderer. La transición LOD conserva fallback básico,
  pero no se encontró una reproducción determinista del pendiente regional; permanece en C.

Tras esos cambios pasan `cargo test-fast --offline`, fmt, Clippy workspace/all-targets y
Clippy WASM con warnings denegados, 33 tests Node y 34 Python. `git diff --check` pasa.
Playwright MCP abrió el host tras el cambio, permitió recrear el visor y mostró el mismo
estado `init failed: no GPU adapter supports this surface`; a 390 px no hubo desbordamiento
horizontal. Solo apareció el 404 de `favicon.ico` en consola. El adapter ausente impide
certificar render, picking o recuperación; C–F y los requisitos de producto siguen abiertos.
