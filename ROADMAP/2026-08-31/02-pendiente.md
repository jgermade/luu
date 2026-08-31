# Lo que queda pendiente

Agrupado por **lo que lo bloquea**, no por tamaño ni por área, porque eso es lo
que decide el orden. Cada punto verificado contra el código de hoy, no contra el
record que lo pidió.

## La deuda de fondo, en una frase

**La última corrida contra un modelo real en este árbol es `d1b0e81`**, del 30 de
agosto. Todo lo posterior —el narrowing, la negativa como mensaje, el `source` de
la puerta, los tombstones, el mapa— se ha verificado contra el mock, a mano o con
un navegador. Las tres son maneras legítimas de comprobar que el código hace lo
que dice. **Ninguna puede decir si ayuda.**

Y hay una asimetría concreta detrás: **ningún modelo ha propuesto nunca un plan
al que este árbol lo haya sujetado.** Todo lo verificado para la puerta, para el
narrowing y para `writes` fue mock o a mano.

## Bloqueado en una máquina con un modelo

Estas tres no son código. Son lecturas, y los instrumentos ya están construidos.

- **La sonda de la puerta.** Quince prompts tecleados por `serve`, qué nombra un
  plan que merezca aprobarse en cada uno, las cuatro maneras en que puede
  ocurrir una negativa y cómo distinguirlas, y los cinco números que apuntar.
  Está escrita en `RECORD/2026-08-31.the-gate-probe.md` y **nunca se ha
  corrido**. Necesita máquina **y una persona en la puerta**, porque el número
  que importa es cuántas veces hubo que enmendar un plan antes de aprobarlo: esa
  tasa de enmienda *es* el coste de la puerta.
  - Tiene que separar, como mínimo: *el modelo nunca declaró una escritura* de
    *el modelo la declaró y `narrow` la rechazó mal*; un plan que nombra un
    fichero que aún no existe (la regla del ancestro existente más cercano es lo
    más ancho de #8 y sólo se ha ejercitado a mano contra `/tmp`); y cuántas
    veces hubo que enmendar.
- **La sonda del mapa.** `grounded.txt` con `--map-tokens 0` contra el mismo
  script con el mapa puesto, muestreo fijado. El mapa cuesta 870 tokens por
  turno; la pregunta es si las respuestas mejoran más de lo que eso cuesta.
  **Nada sobre rankear debería creerse antes de esto.**
- **Réplica en 14b y 32b**, si el fold sigue ganando en tokens a 32K, y si el
  reuse llega a ser *segundos* — lo cual necesita los tiempos de la línea `Done`
  que el backend todavía no lee.

## Bloqueado en nada

- **Rankear el mapa**, y después el grafo de referencias. `@reference.call` está
  en el mismo `TAGS_QUERY` que el outline ya usa; nada en el árbol lo lee. Es lo
  último que le falta al paso 2 y el diferenciador declarado del proyecto. La
  tensión a resolver primero ya está escrita: **rankear personaliza, y un mapa
  que cambia por turno deja de ser un prefijo**.
- **Reconstruir un mapa obsoleto.** Se construye una vez por corrida, así que un
  agente que edita un fichero se queda con el outline que tenía. `mtime` lo
  detecta; *cuándo* pagar la rotura de prefijo es la pregunta de refresh-mode de
  Aider.
- **Leer la ventana de contexto del backend**, en vez de que `--context-limit`
  sea un flag que alguien tiene que acertar. `num_ctx` se manda; no se lee nada
  de vuelta.
- **Sesiones en SQLite.** Verificado: no hay dependencia de sqlite en ningún
  `Cargo.toml`; `serve` pierde la conversación al reiniciar. La regla dura ya
  está fijada: **lo que guarde el store tiene que ser reproducible plegando la
  grabación**, o hay una segunda verdad al lado de la primera.
- **Podar resultados de herramientas** de la historia. No hay `prune` en
  `context.rs`. Sería el primer desalojo **parcial**, y el mensaje `evicted`
  deliberadamente no sabe describir medio turno.
- **Una segunda gramática para el mapa**, cuando haya un segundo lenguaje sobre
  el que preguntar.
- **Backend OpenAI-compatible.** Hoy `BackendKind` es `Mock` y `Ollama` y nada
  más. El trait `Backend` está bien aislado: añadir uno es contenido, no
  rediseño. Es la pieza que más objetivos toca a la vez — ver
  [`04-plano-de-usuario.md`](04-plano-de-usuario.md).

## Narrowing: lo que le queda

- **`network` y `enforcement`.** `Plan` es `steps`, `files`, `writes`,
  `commands` y nada más, así que una tarea sigue heredando los dos del fichero
  de política.
- **Si `writes` debe acotar `run_command`.** Un hijo puede escribir donde las
  raíces de la tarea permitan, y la lista `commands` de un plan no dice nada de
  rutas. Más estrecho de lo que era, y sigue siendo lo más ancho que un plan
  puede pedir.

## Deudas de seguridad, nombradas y sin pagar

- **`openat2(RESOLVE_BENEATH)`** para las herramientas en proceso.
  `sandbox/mod.rs:312` sigue siendo un comentario de documentación nombrando una
  ventana TOCTOU que canonicalizar-y-luego-abrir deja abierta.
- **Nivel 3**, el contenedor, con las restricciones de nivel 2 aplicadas dentro.
- **Auth**, si `serve` llega a escuchar fuera de loopback.

## Pequeñas, y cada una se vuelve folclore si sobrevive

- `luu chat --record` todavía duerme 50 ms antes de salir para que el escritor
  drene (`crates/luu/src/lib.rs:1114`). Una carrera con un retardo pintado
  encima.
- `SessionView` se sigue clonando bajo el mutex en cada lectura de la API
  (`serve.rs:984`, `:992`).
- El replay sigue topando los huecos entre mensajes a 400 ms
  (`ui/store.js:396`) y nada en la UI lo dice.
- `?from=&limit=` en `/turns` sin implementar.
- `RejectTask { task }` no lleva razón, así que un agente rechazado no puede
  proponer otra vez a la luz de lo que el usuario sabía: el usuario reteclea.
  El prompt 10 de la sonda de la puerta es donde esto se va a notar primero.
- **Un turno que nunca estuvo en la ventana se lee igual que uno que sigue en
  ella.** `evicted_by: null` cubre los dos casos, y la llamada de planificación
  es siempre el primer turno de esos. El arreglo honesto es que la vista sepa
  qué turnos tiene la historia.
- Function calling nativo y una gramática GBNF, sustituyendo el parseo de texto.
  La tasa de declaración de la sonda de la puerta es lo que ascendería esto de
  *pequeña* a *siguiente*.
- Sub-tareas y anidamiento. Deliberadamente todavía no.
- `llama-cpp-rs` directo, para controlar el KV cache entre llamadas. Está en el
  diseño como *recomendado* y aplazado desde el principio, hasta que hubiera
  algo que medir.

## Pagado recientemente

Para que la lista no parezca inmóvil:

- **El desalojo como evento grabado.** Pedido tres veces, con la precondición
  *«nada, es pequeño»*, y lo era.
- **La primera pieza de la selección por relevancia** (el mapa), que era la
  única parte no construida del paso 2 desde que el paso 2 se escribió.
- **La negativa como mensaje.** `Refusal::{Busy, Pending, Task, NotGranted}` en
  el cable, con tests de `serve_ws` para cada una.
- **Un test que habría cazado `num_ctx`**: `ollama_wire.rs` comprueba el cuerpo
  de la petición contra un backend de mentira.
