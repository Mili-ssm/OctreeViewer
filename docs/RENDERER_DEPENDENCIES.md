# Migración del renderer a wgpu 30 y glam 0.34 — 14-10-2026

La actualización de manifests/lockfile pendiente dejaba el workspace sin compilar: APIs de
superficie, descriptores, scopes de validación, readback y cámara aún correspondían a wgpu 24 /
glam 0.29. Se completa la adaptación localizada a las dependencias ya seleccionadas:
wgpu/naga 30.0.1, glam 0.34.1, pollster 1.0 y png 0.18. El lockfile también recoge las
actualizaciones transitivas de esa resolución; los checks siguientes prueban ese conjunto.
No se modifica el formato v5/pre1–pre4 ni se cierra la transición LOD por región.

## Contratos adaptados

- Solicitud de adapter mediante `Result` y dispositivo mediante descriptor con valores
  predeterminados para trace/experimental features. Se mantienen límites/presupuestos propios.
- Layouts/buffers opcionales, `immediate_size: 0`, estado de profundidad explícito,
  `multiview_mask: None` y `depth_slice: None` en render passes.
- Los scopes se conservan en un guard hasta `pop().await`; no se descartan antes de comprobar
  la validación del pipeline principal ni del HZB.
- `CurrentSurfaceTexture` distingue éxito/subóptimo, timeout, oclusión, outdated, lost y validación.
  Se presenta con `Queue::present`; subóptimo/outdated reconfiguran sus targets. Oclusión devuelve
  `Suspended` sin suspender permanentemente el renderer. Lost devuelve error explícito que requiere
  recrear superficie/renderer; no se implementa recuperación automática de dispositivo perdido.
- `SurfaceRenderer::render` devuelve ahora `GpuRendererError`, pues wgpu ya no expone el antiguo
  `SurfaceError`. Los strings de estado del binding JS se mantienen.
- `get_mapped_range` es falible: picking y headless propagan errores; un fallo del contador HZB
  mantiene el último conteo y libera el mapeo. La espera nativa usa `PollType` y comprueba su error.
- Cámara con `dcamera::rh::view::look_at_mat4` y
  `dcamera::rh::proj::directx::perspective`: WebGPU exige Y-up y profundidad `[0,1]`.
  Se añade una regresión numérica de near/far y orientación, además de las pruebas de rasterización.

Firmas verificadas en las fuentes locales de las versiones bloqueadas y en la documentación
primaria de [superficie wgpu](https://docs.rs/wgpu/30.0.1/wgpu/enum.CurrentSurfaceTexture.html),
[scopes](https://docs.rs/wgpu/30.0.1/wgpu/struct.ErrorScopeGuard.html) y
[cámara glam](https://docs.rs/glam/0.34.1/glam/dcamera/rh/proj/directx/index.html).

## Validación ejecutada

| Check | Resultado y alcance |
| --- | --- |
| `cargo check --workspace --all-targets --offline` | Pasa tras reproducir el fallo de compilación previo. |
| `cargo test-fast --offline` | 343 pasan, 0 fallan, 4 ignorados: dos pruebas GPU y dos microbenchmarks. |
| Tests GPU ignorados de `viewer_render_wgpu` | Los dos pasan con Vulkan/lavapipe: readback RGBA con filas no alineadas y aristas rasterizadas a dos resoluciones/distancias. Es software, no rendimiento de GPU física. |
| Clippy workspace/all-targets y WASM, `-D warnings` | Pasan; incluye los cambios de tipo de límites GPU en wgpu 30. |
| Build WASM `release`, wasm-bindgen y paquete local | Generados desde las fuentes actuales; `pkg/` y `artifacts/` siguen ignorados por Git. |
| Node del host | 28 tests pasan. |
| unittest Python del host / benchmarks del conversor | 11 / 23 tests pasan. |
| `cargo fmt --all -- --check`, `git diff --check` | Pasan. |

Comandos adicionales al [README](../README.md#desarrollo):

```sh
WGPU_BACKEND=vulkan VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.x86_64.json \
  cargo test -p viewer_render_wgpu --profile fast --offline -- --ignored --nocapture
cargo build --release --offline -p viewer_wasm --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir pkg target/wasm32-unknown-unknown/release/viewer_wasm.wasm
python3 examples/web-host/package-local.py
```

## Navegador: resultado parcial y contraejemplo independiente

Playwright MCP, HeadlessChrome 155, repositorio servido por HTTP en localhost:8000.
La sesión predeterminada anuncia `navigator.gpu`, pero `requestAdapter()` devuelve null.
Una sesión separada con `--enable-unsafe-webgpu --use-angle=vulkan --use-vulkan=swiftshader
--disable-vulkan-surface` permite crear el visor y probar las rutas funcionales.

- `storage-format.browser.mjs`: pasa Blob gris/con acompañante y rechazo de provisional,
  pre5 no soportado y acompañante incorrecto.
- `lod-attributes.browser.mjs`: pasa intensidad/clasificación e identidad de geometría sobre
  la muestra de 4.096 puntos; HZB activo. No se aportó acompañante externo incompatible.
- `lod-session.browser.mjs`: una ejecución inicial abortó con `RuntimeError: unreachable`;
  una ejecución diagnóstica posterior completó sus checks de reemplazo, fallos de respuesta,
  gris, clear/manual y reapertura, pero notificó pérdida de dispositivo. No se considera una
  validación limpia ni se oculta la primera ejecución fallida.
- `lod-budget-fallback.browser.mjs`: no completa; solicitud de adapter falla en esta sesión.
- `presentation.browser.mjs`, modo viewer: no completa el picking/readback; `mapAsync` falla.
  El modo **clear independiente JavaScript**, sin WASM ni HZB, falla con el mismo mensaje:
  `A valid external Instance reference no longer exists.` También notifica `deviceLost`.
  Una segunda configuración ANGLE/SwiftShader reprodujo el fallo del clear.

El contraejemplo impide atribuir esa pérdida exclusivamente a la migración Rust. Tampoco prueba
que la migración esté libre de fallos de navegador: quedan pendientes presentación/picking,
fallback, sesiones sin errores, cámara, móvil y recuperación en los navegadores/dispositivos
objetivo. Los resultados nativos/readback por software no cierran esas prioridades del README.
En el host con la segunda configuración, «Open built-in sample» llega a 4.096 vóxeles,
cobertura completa, cero bloques pendientes y HZB activo. La captura de escritorio continúa
blanca: ese estado funcional no acredita presentación. Se observó además un 404 de recurso
al abrir la raíz del servidor; no se atribuye a la migración ni se omite de la revisión de consola.
No se publican FPS, ahorro de memoria ni mejoras de rendimiento por actualizar dependencias.
