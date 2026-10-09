# Muestras de nubes de puntos

Datasets pequeños para probar importación, conversión LAS/LAZ y el formato `.oct`. Los `.oct`
incluidos se generaron con la CLI del proyecto y se validaron al finalizar la conversión.

| Archivo | Contenido | Puntos | Fuente |
| --- | --- | ---: | --- |
| `fs_e432_n4002_18.las` / `.oct` | Costa de Outer Banks, Carolina del Norte (USGS, post Nor'Ida 2009) | 149.730 | [USGS DS 616](https://pubs.usgs.gov/ds/616/) |
| `fs_e430_n4008_18.las` / `.oct` | Otra tesela costera de Outer Banks del mismo levantamiento | 111.190 | [USGS DS 616](https://pubs.usgs.gov/ds/616/) |
| `stanford_bunny_bun000_unstructured.las` / `.oct` | Vista parcial de un conejo escaneado, 0° (Stanford Range Repository) | 40.256 | [PCD de PCL en SourceForge](https://sourceforge.net/projects/pointclouds/files/PCD%20datasets/registration/stanford_bunny.zip/download) |
| `stanford_bunny_bun000_unstructured.pcd` | Fuente PCD original de esa vista | 40.256 | [PCD de PCL en SourceForge](https://sourceforge.net/projects/pointclouds/files/PCD%20datasets/registration/stanford_bunny.zip/download) |

Los datos del USGS son del levantamiento EAARL de topografía costera de Outer Banks después del
huracán Nor'Ida; la ficha del conjunto y sus metadatos están en el [sitio de la publicación](https://pubs.usgs.gov/ds/616/).
El USGS indica que sus datos producidos por la agencia están en el dominio público. Atribución
sugerida: Bonisteel-Cormier et al. (2011),
*EAARL Coastal Topography—Northern Outer Banks, North Carolina, Post-Nor'Ida, 2009*, USGS Data
Series 616. Véase la [política de derechos del USGS](https://www.usgs.gov/information-policies-and-instructions/copyrights-and-credits).

El objeto procede de un escaneo óptico Cyberware 3030MS del [Stanford Range Repository](https://graphics.stanford.edu/software/scanview/models/bunny.html).
La vista PCD se conserva como fuente; el LAS se preparó para este visor con coordenadas XYZ y una
intensidad uniforme de 45000 para que el importador actual la muestre en gris. Por tanto, ese gris es
sintético, y el LAS no conserva normales ni color medido. Los [términos de Stanford](https://www.stanford.edu/terms/)
limitan el uso de sus materiales a fines personales y no comerciales; el [registro de PCL](https://sourceforge.net/projects/pointclouds/files/PCD%20datasets/registration/stanford_bunny.zip/download)
no declara una licencia específica para estos datos. Los archivos locales de esta vista no se
versionan ni se redistribuyen; el `.gitignore` del directorio los mantiene fuera de Git.

Para regenerar los `.oct` desde LAS:

```bash
cargo run --release -p viewer_convert -- data/samples/fs_e432_n4002_18.las data/samples/fs_e432_n4002_18.oct
cargo run --release -p viewer_convert -- data/samples/fs_e430_n4008_18.las data/samples/fs_e430_n4008_18.oct
cargo run --release -p viewer_convert -- data/samples/stanford_bunny_bun000_unstructured.las data/samples/stanford_bunny_bun000_unstructured.oct
```

El [Stanford Bunny](https://graphics.stanford.edu/software/scanview/models/bunny.html) es una
reconstrucción conocida a partir de escaneos de rango, con zonas sin cobertura. La vista parcial
incluida resulta útil para comparar una forma de objeto con los paisajes costeros.
