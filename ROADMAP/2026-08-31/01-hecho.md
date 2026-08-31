# Lo que está hecho

Verificado contra el árbol en `8a4e80b`, no contra los records que pidieron cada
pieza.

## El árbol hoy

- **14 commits**, del 26 al 31 de agosto de 2026. Seis días.
- `cargo fmt --all --check` limpio.
- `RUSTFLAGS=-D warnings cargo clippy --workspace --all-targets` limpio.
- **188 tests verdes**: 155 unit en `agent-core`, 15 unit en `luu`, 14 en
  `serve_ws`, 4 en `ollama_wire`. Más **4 Playwright** en el job `web`.
- Dos crates: `crates/agent-core` (el motor, que no sabe nada del CLI ni del
  navegador) y `crates/luu` (el binario, el servidor y la UI embebida).
- `cargo build` no necesita node, que es una restricción declarada y sostenida:
  la UI son ficheros sueltos servidos con `rust-embed`.

## Commit a commit

| Commit | Qué cerró |
| --- | --- |
| `f207f96` | El diseño inicial del agente |
| `725f5de` (#1) | Esqueleto andante: protocolo, `serve` y el cliente web |
| `6a26841` (#2) | El gestor de contexto: la historia como tipo, un presupuesto de tokens real |
| `ee12beb` (#3) | Plan: medir el reuse de prefijo primero, desalojar por bloques después |
| `9b241e4` (#4) | Herramientas y sandbox: el kernel sujeta al subproceso |
| `5de89e2` (#5) | Tareas: la puerta, el fold y la aprobación |
| `f588d4a` (#6) | Fragmentos: fusionar ficheros reales en el prompt a través del sandbox. Se manda `num_ctx`. Se miden **todas** las llamadas de un turno |
| `16b45d0` | Muestreo fijado (`--temperature`, `--seed`) y la sonda del fold **corrida**: pierde 2 de 4 respuestas |
| `1862400` (#7) | El resumen de una tarea cerrada cita sus fragmentos verbatim — el arreglo de lo anterior |
| `d1b0e81` | El par vuelto a correr con una sola variable de diferencia: los turnos 17 y 18 se recuperan |
| `ddd869a` (#8) | Narrowing: el plan aprobado **es** el sandbox de su tarea. `writes` en el plan, `Authority` en cada veredicto, `Refusal` en el cable (protocolo v2) |
| `5a04beb` (#9) | Recuento: lo que cerraron cinco commits y lo que ningún modelo ha visto |
| `e7bb400` | Tombstones de desalojo: `evicted` en el protocolo (v3, formato de grabación 5) |
| `c3ec4db` | La sonda de la puerta, escrita antes de correrla. `source` en `task_proposed`; el plan *como se propuso* junto al aprobado |
| `e95ebf5` / `8a4e80b` (#10) | El mapa del repositorio: `tree-sitter` saca el outline de cada `.rs` al prefijo cacheado |

## El orden de trabajo del diseño

| Paso | Estado |
| --- | --- |
| 1. `agent-core`: tipos base + backend | **Hecho** |
| 2. Gestor de contexto | Casi: falta **rankear** el mapa |
| 3. Protocolo + `serve` + cliente de depuración | **Hecho**, v3 en el cable |
| 4. Sandbox de rutas y comandos | **Hecho**, política por tarea incluida |
| 5. Empaquetado en contenedor (nivel 3) | Sin empezar |
| 6. Extensión de VSCode | Sin empezar, deliberadamente la última |

El paso 4 fue lo primero en la lista de todos los recuentos anteriores. Es la
primera vez que esa tabla no tiene ningún *«comprobado, no aplicado»*.

## Pieza a pieza

### Gestión de contexto — el diferenciador

- **La historia es un tipo**, no una cadena. `Context::select` elige contra un
  presupuesto de tokens y el renderizado es una función pura de esa elección,
  así que cada token enviado es atribuible a un bucket.
- **Dos políticas de desalojo**: `turn` suelta el mínimo, `block` corta hasta
  `--low-water` y luego se está quieto. Sobre los mismos veinte prompts a 1024
  tokens: diez cortes pequeños con `turn`, dos profundos con `block`, y
  **ninguno** en la corrida con tareas, porque el fold mantuvo la historia por
  debajo del límite.
- **El desalojo es un evento**, no sólo un efecto: `evicted` va en el protocolo
  al lado de `task_closed` y nombra el turno que cortó, los turnos que se
  fueron, lo que valían, qué contador los midió y qué política cortó.
- **Compactación en frontera de tarea (el fold)**: una tarea cerrada se
  sustituye por un resumen determinista —el plan aprobado, lo que reportaron los
  resultados de herramientas, y los fragmentos que se le enseñaron, citados
  verbatim bajo un tope de tokens—. Nunca prosa del modelo: entraría en la
  región de escritura-única sobre la que se construye cada turno posterior.
- **Fragmentos** (`--fragment PATH[:START-END]`, `## fragment:`): un fichero
  real leído **a través del sandbox** y fusionado en **un** turno. Una ruta que
  el sandbox rechaza es un error, no un aviso.
- **Mapa del repositorio** (`--map-tokens N`, `luu map`): las definiciones de
  cada `.rs` con sus firmas y los cuerpos elididos, del `TAGS_QUERY` propio de
  `tree-sitter`, colocado bajo las definiciones de herramientas y sobre la
  historia. **Apagado por defecto**, porque encendido cambiaría todos los
  números de todas las grabaciones anteriores.
- **Cada cuenta de tokens dice qué contador la produjo.** Sin `--tokenizer` son
  `chars/4` y lo dicen en todas partes donde aparecen.

### Tareas y la puerta

- Un prompt sin tarea abierta compra una llamada de planificación y luego queda
  **retenido, sin correr**, hasta que se aprueba o se rechaza en la UI.
- **El plan escrito es la aprobación**: cada fichero que nombra tiene que ser
  alcanzable en el sandbox resuelto y cada comando permitido, o la corrida para
  antes del primer turno.
- **El plan aprobado es el sandbox de su tarea.** `files` concede lectura,
  `writes` lectura-escritura; un plan que no declara escrituras no puede
  escribir. El fichero de política es el límite exterior y un plan que no nombra
  nada no concede nada; una negativa dice cuál de los dos rechazó.
- El ciclo de vida es una máquina de estados y **cada transición está guardada**:
  una propuesta no se puede cerrar, un plan rechazado no se puede reabrir.
- Una propuesta dice **quién la escribió**: `source` es `model` cuando la llamada
  de planificación emitió un bloque de plan parseable, `prose` cuando contestó
  sin él (el caso ordinario de un 7B) y `written` cuando vino de un `## task:`
  de un script.
- El lado de lectura guarda el plan **como se propuso** junto al plan aprobado:
  la diferencia entre ambos es lo que una persona tuvo que añadir, que es el
  coste de la puerta.

### Sandbox

- Cinco herramientas: `read_file`, `write_file`, `edit_file`, `list_dir`,
  `run_command`.
- **El modelo nunca ejecuta nada.** Emite una petición estructurada; el programa
  la parsea, la valida contra la `SandboxPolicy` y ejecuta código Rust real.
- **Un subproceso es del kernel, no nuestro**: `run_command` construye un
  ruleset de Landlock y un filtro seccomp en el padre y los aplica en
  `pre_exec`. Donde el kernel no puede (macOS), el defecto es **denegar**, no
  correr el hijo sin sujetar.
- **Nada puede decir «sandboxed» sin decir por quién.** Cada veredicto lleva
  `Authority`, y uno parcial lleva qué falta.
- Las rutas se canonicalizan antes de comparar, o un symlink sale del sandbox
  andando.
- `luu tools` imprime el sandbox resuelto y los bytes exactos del prefijo.

### Protocolo, servidor y cliente

- **Un solo esquema JSON, varios transportes**: stdio, WebSocket y el formato de
  grabación llevan los mismos enums. Las trazas de depuración van detrás de
  `--trace`, en su propio canal.
- Protocolo **v3**, formato de grabación **5**. Cada variante nueva de un enum
  etiquetado sube la versión, porque es un cambio que un lector viejo no puede
  parsear.
- `serve` en `127.0.0.1:7878`: `/ws`, `/ws/trace`, y una API de lectura
  (`/api/sessions`, `/turns`, `/prompt`, `/context`).
- La UI: composer, transcripción con las tareas cerradas plegadas al resumen que
  el modelo ve ahora —expandibles a lo que ya no ve—, la puerta completa
  (aprobar añadiendo `files`/`writes`/`commands`, rechazar, cerrar, reabrir),
  cancelación, y los paneles de presupuesto de tokens, reuse de prefijo,
  llamadas a herramientas y prompt enviado.
- `--record` escribe una sesión reproducible desde `chat` o desde `serve`;
  `luu export` la convierte en el gemelo estático de la API de lectura, que es
  lo que hace que el despliegue en Pages sea más que una captura.

### Los tests que corren la cosa

- `crates/agent-core/tests/ollama_wire.rs` pone un servidor HTTP de mentira en
  un puerto efímero y comprueba **qué manda `Ollama::stream` de verdad**,
  incluida la ventana: ese bug ya se coló una vez.
- `crates/luu/tests/serve_ws.rs` levanta el servidor, conduce la puerta por
  `/ws` y comprueba que la API de lectura dice lo mismo que dijo el socket.
- Playwright carga el sitio montado, reproduce una grabación, pincha todos los
  fixtures y falla ante cualquier error de consola. Ya se ganó el sueldo:
  encontró un replay doble que enseñaba un turno fantasma a todo el que visitaba
  la página desplegada.

## Lo que se ha medido de verdad

Con `qwen2.5-coder:7b` y muestreo fijado, dos corridas del par grounded:

| Turno | Pregunta | Historia completa | Con tareas, antes del fix | Con tareas, después |
| ---: | --- | --- | --- | --- |
| 16 | los dos compromisos, `context.rs` | ✓ | ✓ | ✓ |
| 17 | los tres trabajos de la frontera, `task.rs` | ✓ | ✗ perdida | ✓ recuperada |
| 18 | los cinco programas permitidos, `luu.toml` | ✓ | ✗ negativa | ✓ recuperada |
| 19 | `enforcement = "kernel"` sin Landlock | ✗ | ✗ | ✗ en los dos lados |

Y lo que costó: la corrida con tareas pasó de **49,95%** de los tokens de la
corrida plana a **80,3%** una vez que los resúmenes citan sus fragmentos. Una
quinta parte de victoria en vez de la mitad, y la quinta parte es real.

Otros números que están medidos y son citables:

- El outline completo de este repositorio son **6 327 tokens: el 77% de una
  ventana de 8K**. A cualquier presupuesto que una corrida real se pueda
  permitir, la mayor parte del repositorio no está en el mapa, y **el orden
  alfabético eligió cuál**.
- El mapa cuesta **870 tokens por turno** a `--map-tokens 1024`, un **+56%**
  sobre el total del script grounded.
- El reuse de prefijo sube de 93,9% a 96,3% con el mapa puesto **por aritmética
  pura**: un bloque constante más grande sube el compartido y el total a la vez.
  Un número de reuse con mapa no es comparable con uno sin mapa.
