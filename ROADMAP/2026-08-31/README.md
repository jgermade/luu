# La foto del 31 de agosto de 2026

Una foto fechada del proyecto: qué está hecho, qué queda pendiente y qué queda
por decidir, más dos diagramas de Gantt con esa misma información en el tiempo.

## Qué es esta carpeta, y qué no

El repositorio ya tiene dos sitios donde vive la verdad, y esta carpeta no
sustituye a ninguno:

- [`loude-design.md`](../../loude-design.md) es **el diseño tal y como está
  ahora**. Se reescribe cuando una decisión cambia, así que siempre se lee como
  la respuesta actual.
- [`RECORD/`](../../RECORD/) es **cómo se llegó a esa respuesta**: propuestas
  fechadas, append-only, incluidas las que estaban mal primero.

`ROADMAP/<fecha>/` es una tercera cosa, más tonta a propósito: **una foto para
alguien de fuera**. No decide nada, no propone nada y no es fuente de verdad
para nada. Si algo aquí contradice al diseño, el diseño gana y esta carpeta está
caducada. Como las de `RECORD/`, no se edita: cuando la foto cambie, se hace una
carpeta nueva con otra fecha.

Está en castellano, a diferencia del resto del repositorio, porque se escribió
como material de conversación y no como documentación del proyecto.

## Índice

| Fichero | Qué contiene |
| --- | --- |
| [`01-hecho.md`](01-hecho.md) | Lo entregado, commit a commit y pieza a pieza, con los números que se midieron |
| [`02-pendiente.md`](02-pendiente.md) | Lo que falta, agrupado por **lo que lo bloquea**, que es lo que decide el orden |
| [`03-por-decidir.md`](03-por-decidir.md) | Las decisiones abiertas: las del diseño, las del plano de usuario y la de rumbo |
| [`04-plano-de-usuario.md`](04-plano-de-usuario.md) | Los cuatro entornos —CLI, VSCode, web local, web remota— y a qué distancia está cada uno |
| [`gantt-motor-y-superficie.html`](gantt-motor-y-superficie.html) | Los dos Gantt en una sola página: el motor y la superficie |

## Cómo leer los Gantt

Es un fichero suelto: se abre en un navegador sin servidor ni build. Contiene
los dos diagramas.

- **El eje está partido.** A la izquierda de la línea vertical, seis días a
  resolución de día (26–31 ago) con **fechas de commit reales**. A la derecha,
  semanas. Sin la partición, todo el trabajo hecho ocuparía 1/20 del ancho.
- **Todo lo que hay a la derecha de esa línea es una estimación**, no está
  escrita en ningún sitio del repositorio y no la respalda nadie. Las
  *dependencias* sí salen del código y de `RECORD/`; el *calendario* no.
- **Los dos diagramas no se suman.** Cada uno cuenta sus semanas desde el
  arranque de su vía, porque compiten por la misma persona. Las barras rayadas
  son la misma pieza apareciendo en ambos: se hace una vez.
- Violeta = una corrida contra un modelo, no código. Rombo = una decisión, que
  es un instante y no una duración. Borde discontinuo = ni construido ni
  decidido ni presente en el diseño.

## Procedencia

Lo de la columna «hecho» está verificado contra el árbol en `8a4e80b`: los
`Cargo.toml`, los ficheros de `crates/`, las rutas de `serve.rs` y el `clap` de
`crates/luu/src/lib.rs`, no contra el record que pidió cada cosa. Los números
medidos (reuse, tokens, tests) salen de los records que los produjeron y están
citados donde aparecen.
