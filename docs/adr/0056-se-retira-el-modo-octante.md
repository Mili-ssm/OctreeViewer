# 0056 Se retira el modo octante: el modo celda es el único

Estado: Aceptada · Fecha: 18-11-2026 · Decide: propietario (retirada y momento; los detalles de API de «Decisión» los fijó el agente) · Sustituye al [ADR 0034](0034-nivel-por-octante-con-arista-real.md) en lo que quedaba vigente; precisa [ADR 0050](0050-nivel-por-celda-segun-distancia.md) y [ADR 0055](0055-la-gpu-decide-el-nivel-la-cpu-la-residencia.md)

## Contexto
- El visor tiene dos modos de selección de nivel: por octante (`LodMode::Octant`, la CPU elige un nivel por octante
  de nodo) y por celda (`LodMode::Cell`, la GPU elige por celda, ADR 0050 y 0055). El modo octante es el
  predeterminado de la librería y el modo celda el de la demo.
- Mantener los dos cuesta más de 27 ramas `LodMode`, una ruta propia en el shader y tests duplicados; y obliga al
  modo celda a alimentar la maquinaria por octante compartida.
- El modo octante conserva dos huecos conocidos ([#59](https://github.com/Mili-ssm/OctreeViewer/issues/59)) que no
  se iban a arreglar.
- El propietario condicionó la retirada a validar la fase 2b en el navegador
  ([OQ-12](../requirements.md#preguntas-abiertas)) y la dio por validada el 18-11-2026.
- Los crates son internos y no se publica nada antes de la 1.0 ([ADR 0026](0026-api-publica-licencia-y-version-1.md)); no hay promesa
  de compatibilidad de `setCellLod`.

## Decisión
- **El modo celda es el único modo y el predeterminado de la librería.** Se borra `LodMode::Octant` y, al quedar
  una sola variante, el propio `LodMode`.
- **API:** se borran `setCellLod` (wasm, worker y `viewer-api.mjs`), el selector «Level selection» de la demo y la
  opción `--cell-lod` de `cell_pool_bench`. `?cellLod=` se ignora con un aviso en consola, para que los enlaces
  antiguos a la demo sigan abriendo.
- **`cell_cut`**, la función pública de corte, pasa a calcular en modo celda; no se borra, porque la usan
  `coverage_spike` y los bancos.
- **La opción `ActiveRule`** (ventana o contiguo) no cambia: la decide el propietario ([#66](https://github.com/Mili-ssm/OctreeViewer/issues/66)).
- **#59 se cierra sin arreglo**: el defecto desaparece con el modo.
- La maquinaria por octante que el modo celda todavía usa (pasos, sondas, retención por nodo) **no** se borra aquí:
  la sustituye el [plan del ADR 0055](../plans/adr-0055-rango-por-nodo.md).

## Alternativas
- **Conservarlo congelado**: dos rutas y #59 abierta sin fecha; es lo que había.
- **Conservar `setCellLod` como operación sin efecto**: API muerta antes de la 1.0; un error claro al usarla es
  mejor que un silencio, y borrarla da ese error.
- **Borrar también `cell_cut`**: arrastra `coverage_spike`, que reproduce las poses de los registros.

## Consecuencias
- Los tests que no fijaban modo corrían en modo octante: pasan a modo celda, y los que comprueben algo que sigue
  importando se portan en vez de borrarse ([plan](../plans/retirada-del-modo-octante.md)).
- Las cifras por octante de los registros anteriores dejan de ser reproducibles; se guarda antes una línea base
  del modo celda.
- El plan del ADR 0055 se simplifica: ya no hay código compartido que mantener alimentado (su decisión D4).
- Requisitos y explicaciones que describen el modo octante como vigente se actualizan en el mismo trabajo.

## Revisar si
- Un requisito pide un nivel por octante, o aparece un usuario externo de `setCellLod`.
