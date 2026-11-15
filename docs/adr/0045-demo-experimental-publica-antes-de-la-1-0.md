# 0045 La demo web se publica como demo experimental antes de la 1.0

Estado: Aceptada · Fecha: 15-11-2026 · Decide: propietario (delegó la decisión el 15-11-2026) · Acota CON-05 de [requisitos](../requirements.md#restricciones)

## Contexto
- CON-05 ([ADR 0026](0026-api-publica-licencia-y-version-1.md)) dice que nada se publica, ni npm ni app
  pública, hasta la 1.0.
- El 15-11-2026 el repositorio pasa a ser público como escaparate experimental del propietario (CV). El
  propietario dejó la demo a criterio del agente: «hasta la 1.0 no lo haremos 100 % formal».
- No hay CI ([CONTRIBUTING](../../CONTRIBUTING.md#checks)): todo se construye en local.

## Decisión
- La demo de `examples/web-host` se publica en GitHub Pages, marcada como **experimental y pre-1.0**,
  con los datos de muestra pequeños versionados (sin datasets de terceros sin licencia de redistribución).
- Se construye y se sube en local con un script a la rama `gh-pages`; no hay despliegue automático ni workflows
  propios. GitHub Pages publica siempre con su ejecución gestionada de Actions: Actions se activa solo para eso
  (acciones de GitHub), sin coste en un repositorio público.
- CON-05 sigue vigente para npm y para una app pública «oficial»: la demo no es la app de la 1.0 ni una
  API estable.

## Alternativas
| Opción | Por qué no |
| --- | --- |
| Solo demo local hasta la 1.0 | Quien llegue desde el CV no puede verla sin compilar Rust y wasm |
| Despliegue automático con Actions | El propietario no quiere CI |

## Consecuencias
- La portada enlaza la demo y avisa de que es experimental y necesita Chrome o Edge con WebGPU.
- Publicar una versión nueva de la demo es un paso manual; puede quedarse por detrás de `main`.

## Revisar si
Llega la 1.0 (la demo pasa a construirse sobre la API pública, CON-05), o la demo necesita datos o
cabeceras que GitHub Pages no sirve.
