# Octree Viewer

Visor experimental de nubes de puntos en Rust, con un núcleo común, una capa de preparación de
escena `wgpu` y bindings WASM.

## Estado actual

El núcleo recibe puntos RGB en lotes y los agrega en voxeles dentro de un `HashMap` en memoria. Se
añadió un lector LAS/LAZ incremental que convierte puntos a coordenadas locales y devuelve lotes
acotados (hasta 65.536 puntos, con un límite adicional basado en el tamaño de registro). Esto limita
las copias de salida, pero no sustituye una medición de memoria del decodificador LAZ con archivos
reales. En navegador, `WasmLasImportSession` lee un `Blob` usando `FileReaderSync`; debe crearse
dentro de un Web Worker dedicado y el host debe pedir el siguiente lote solo cuando pueda procesarlo.
El host web todavía envía arrays a WASM y recibe una copia de todas las instancias para dibujarlas
como una proyección 2D de depuración. La capa `viewer_render_wgpu` prepara datos y define un shader,
pero todavía no crea un dispositivo, un pipeline ni presenta una escena en GPU.

La lectura LAS/LAZ por bloques está disponible, pero no hay almacenamiento fuera de memoria, octree
ni selección por nivel de detalle, y la importación aún no alimenta un visor GPU. Por tanto, el
proyecto todavía no puede visualizar nubes de cientos de GB de extremo a extremo. El tamaño del
archivo no determina por sí solo el coste: también cuentan la cantidad de puntos y voxeles únicos,
el formato, la memoria disponible y el presupuesto de renderizado.

## Dirección de evolución

1. Conectar la sesión incremental LAS/LAZ al host web mediante un worker con cancelación y control
   de contrapresión.
2. Construir un índice espacial jerárquico fuera de memoria, con niveles de detalle y consultas por
   frustum; conservar una ruta de voxelización para agregación y reducción de densidad.
3. Enviar a GPU solo los bloques visibles dentro de un presupuesto de instancias y memoria.
4. Medir tiempo de carga, memoria máxima, latencia de consulta y estabilidad visual con datasets
   representativos, antes de fijar objetivos cuantitativos.

## Desarrollo

```bash
cargo test --workspace
cargo fmt --all -- --check
cargo check -p viewer_wasm --target wasm32-unknown-unknown
```

En el entorno actual se pueden ejecutar los tests de escritorio en modo offline si las dependencias
están en caché. La compilación WASM también requiere tener instaladas las dependencias del target.
