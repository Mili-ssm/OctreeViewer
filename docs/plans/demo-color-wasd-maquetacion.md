# Demo: color estable, WASD, maquetación y segunda nube

Estado: en curso · 19-11-2026 · petición del propietario del 19-11-2026; rama `feat/demo-color-wasd-layout`

**Conclusión.** Cuatro arreglos de la demo pedidos por el propietario y, después, el pendiente #89. El color
por elevación e intensidad deja de depender de la vista; el WASD pasa a moverse respecto a la cámara, de forma
continua y a una velocidad proporcional a la escena; el visor ocupa la pantalla y solo el menú hace scroll; y,
si cabe, la demo ofrece una segunda nube real (un recorte de Belvedere).

## Causas encontradas (19-11-2026)

| Síntoma | Causa | Dónde |
| --- | --- | --- |
| La saturación del color cambia con la misma nube | Los rangos de elevación e intensidad se fusionan cada frame solo sobre los niveles **dibujados**: dependen de la cámara y del LOD | `cell_driver.rs`, `CellColorRanges` y su fusión antes de `set_color_ranges` |
| El WASD avanza «1 mm» y va raro | Cada `keydown` mueve `velocidad × 0,05` unidades del dataset (5 cm con velocidad 1, sin escalar a la escena), en **ejes del mundo** (W = +Y) y al ritmo de la autorrepetición del teclado; solo en modo pájaro | `main.js` y `viewer-api.mjs` (`keyDown`); `OrbitCamera::translate` |
| Navegar el menú es incómodo | `body` es una rejilla con `min-height: 100vh` y el lienzo mide `100vh - 48px`: hace scroll la página entera | `index.html` (CSS) |

## Decisiones

| Decisión | Por qué |
| --- | --- |
| Elevación: rango fijo por dataset, el eje Z de la extensión de los datos (`stats.data_bounds`) | La misma nube da el mismo color desde cualquier vista; la extensión es exacta y ya está en los metadatos |
| Intensidad: unión de los rangos de todos los niveles almacenados desde que cambió el dataset o la generación de color; nunca se estrecha | El formato no guarda un rango de intensidad fiable (los rangos de columna salen `[0, 0]`); la unión solo crece al llegar datos nuevos y no cambia al mover la cámara sobre lo ya cargado |
| La paridad de color de R3-Q32 no cambia | Habla de la función de color con los mismos rangos, no de cómo se eligen |
| WASD respecto a la cámara (adelante y lado en el plano horizontal según el giro; Q/E vertical), continuo mientras la tecla está pulsada, con `dt` por frame, y en los dos modos de cámara | Es lo que espera un usuario de un visor 3D; la autorrepetición no da movimiento continuo |
| Velocidad proporcional a la distancia de la cámara al objetivo; la rueda en modo pájaro sigue siendo el multiplicador | La misma tecla se nota igual en Majadas y en la muestra |
| Majadas no entra en la demo | Su registro no declara licencia (`data/datasets/README.md`) y ocupa GB; GitHub Pages admite 100 MB por fichero |
| Segunda nube: recorte de Belvedere (CC BY 4.0, RGB real), ≤ 40 MB entre `.oct` y `.octattrs` (lo comprueba el script), con atribución en la página | Datos reales con licencia que permite publicarlos |

## Tareas

- [x] **1. Color estable.** Comprobar: un test del driver con dos cortes distintos del mismo dataset da los
  mismos rangos; en el navegador, el modo elevación no cambia de color al girar y acercarse.
- [x] **2. WASD.** Comprobar: tests de la cámara (W avanza en la dirección de la vista proyectada en el plano);
  en el navegador, mantener W mueve de forma continua y visible en la muestra y en Belvedere.
- [x] **3. Maquetación.** Comprobar con Playwright en escritorio y móvil: el lienzo ocupa la ventana sin scroll
  de página, el menú hace scroll propio y el teclado y el foco siguen funcionando.
- [x] **4. Segunda nube (prescindible).** Comprobar: el recorte se convierte y verifica
  (`viewer_convert verify`), la demo lo abre desde un selector, la atribución aparece y los ficheros caben.

Hecho (19-11-2026): 1 en `ab392d9` y `b681853` (el respaldo del cubo raíz estiraba la rampa: el worker pasa
`elevationRange`); 2 en `43855b8` y `e38c013` (0,3 radios de órbita por segundo: a 1 radio por segundo, 1 s de W
sacaba la muestra de la vista); 3 en `5a2d51f`; 4 en `f41798c`, `b390a64`, `f26de18` y `ee5734e` (600 × 600 m,
1,32 M puntos, 21 MB; se descarga entera porque el servidor de desarrollo no admite Range). Navegador: maquetación
en escritorio y móvil, WASD en los dos modos y Belvedere pasan; la prueba del color con la rueda no reproduce el
fallo en la compilación anterior, así que la evidencia del arreglo es el test del driver
`the_frame_hands_the_target_ranges_that_do_not_depend_on_the_cut`.

- [ ] **5. #89, lints pedantes.** Comprobar: `scripts/check.sh` 23/23.
- [ ] **6. Cierre.** Comprobar: `scripts/check.sh` y el navegador en la punta; PR; fusión y publicación de la
  demo solo con el visto bueno del propietario.
