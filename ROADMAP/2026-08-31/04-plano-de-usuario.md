# El plano de usuario: los cuatro entornos

Los cuatro sitios donde alguien usa esto, y a qué distancia está cada uno. La
distancia es un juicio, no una medida; lo que hay debajo de cada juicio está
verificado contra el código.

**El resumen en una frase:** lo construido es el motor y su instrumento de
medida, no un producto. Ninguno de los cuatro entra en el orden de trabajo antes
del paso 5, y el cuarto no está en el diseño en absoluto.

| Objetivo | Distancia | Qué la decide |
| --- | --- | --- |
| CLI tipo opencode | Media | Hay motor; no hay sesión interactiva ni proveedores |
| Extensión de VSCode | Lejos, pero barata | Cero código; el protocolo ya existe, el transporte no |
| Web local | Media-corta | Existe y funciona, pero es un cliente de **depuración** de una sola sesión |
| Web remota securizada | Muy lejos | No existe, y no está diseñado |

---

## 1. CLI, al nivel de opencode

### Lo que ya hay

Lo difícil: el bucle de herramientas con cinco herramientas, el sandbox que
sujeta al hijo con Landlock y seccomp, las tareas con el plan como aprobación
*y* como sandbox, el streaming a stdout, y la contabilidad de contexto que
ningún CLI de estos tiene. Cinco subcomandos: `chat`, `serve`, `tools`, `map`,
`export`.

### Lo que falta

- **No hay sesión interactiva.** `luu chat` es one-shot (argumento o stdin) o
  `--script`. No hay REPL ni TUI. opencode es una sesión conversacional; esto no.
- **Un solo proveedor.** `BackendKind` es `Mock` y `Ollama`. No hay endpoint
  OpenAI-compatible —que es la pieza que de un golpe daría LM Studio,
  `llama-server`, vLLM y cualquier host remoto— ni `llama-cpp-rs` directo.
- **No hay descubrimiento ni selección de modelo.** Modelo, URL y ventana son
  flags con `qwen2.5-coder:7b` por defecto.
- **No hay config de usuario.** `luu.toml` es sólo `[sandbox]`; no hay perfiles
  de proveedor ni nada en `~/.config`. Todo va por flags.
- **No hay reanudar.** Cada `chat` empieza de cero; `--record` escribe y nadie
  lo relee.
- **No hay puerta en el CLI**, y si debe tenerla sigue sin decidirse.
- Nada de MCP, subagentes ni LSP.

---

## 2. Extensión de VSCode, junto al CLI o por su cuenta

### Lo que ya hay

**Cero líneas de TypeScript** — es el paso 6, deliberadamente el último. Pero la
mitad del trabajo está hecha en otro sitio: el protocolo v3 es un enum JSON
versionado, agnóstico del transporte, y `agent-core` no sabe nada del CLI ni del
navegador. Eso es lo que convierte el aislamiento en contenedor y la extensión en
preguntas de empaquetado en vez de reescrituras.

Además, el paso de confirmación de una tarea encaja solo en la UX de la Chat API:
un plan editable antes de ejecutar, luego ejecución con streaming de qué
herramienta se está usando, y una tarea cerrada que se pliega a su resumen en el
hilo.

### Lo que falta

- **El transporte stdio no existe.** El diseño y los comentarios de
  `protocol.rs` dicen «stdio (the VSCode bridge)», pero lo único implementado es
  `/ws` en `serve.rs`. `chat` escribe a stdout en su propio formato, no el
  protocolo.
- **`serve` es de una sola sesión** (un `view` bajo un mutex; `/api/sessions`
  devuelve una lista de un elemento). Una extensión con dos chats abiertos no
  encaja hoy.
- La extensión en sí: el bridge en TypeScript sobre `vscode.chat.createChatParticipant`.

---

## 3. Web local, equivalente al CLI

### Lo que ya hay

La más cerca de las cuatro, y con una trampa. Ya existe, se usa y está testeada
(14 tests sobre el socket, 4 de Playwright): composer, transcripción con las
tareas plegables, la puerta completa —aprobar añadiendo `files`/`writes`/`commands`,
rechazar, cerrar, reabrir—, cancelación, replay de grabaciones, y los paneles de
presupuesto, reuse, llamadas a herramientas y prompt enviado.

**La trampa:** está construida como instrumento de medida, no como producto. Los
paneles pesan más que el chat, y eso fue una decisión, no un descuido — la UI
existe para producir los números con los que se compara una estrategia de
contexto contra otra.

### Lo que falta para ser «equivalente al CLI»

- **Multi-sesión**: crear, cambiar, borrar. Hoy hay una.
- **Persistencia**: reiniciar `serve` pierde la conversación. Bloqueado en nada.
- **Elegir proveedor, modelo y ventana en caliente**: hoy se fijan al arrancar.
- `?from=&limit=` en `/turns` sin implementar, y el tope de 400 ms del replay
  que nada en la UI menciona.
- La decisión de reencuadre: instrumento, producto, o dos modos.

---

## 4. Web remota securizada, multiusuario y multi-host

### Lo que ya hay

Poco, y conviene decirlo entero: **no se ha empezado, y tampoco está decidido**.
`loude-design.md` no menciona auth, ni usuarios, ni multi-host. Lo único escrito
es una línea en deudas de seguridad: *«auth, si `serve` se expone fuera de
loopback»*.

Lo que juega a favor:

- El protocolo ya viaja por WebSocket y es el mismo en los tres transportes.
- `--ollama-url` ya apunta a cualquier host, que es lo más parecido a
  «remotizar» que existe hoy.
- El nivel 3 está diseñado —binario estático con `musl`, imagen mínima,
  bind-mount sólo de lo permitido, red desactivada por defecto, y el backend de
  inferencia en el host hablando por un socket expuesto a propósito si hace
  falta la GPU.

### Lo que falta

Todo lo demás: TLS, usuarios, sesión por usuario, autorización cruzada con el
sandbox, registro y enrutado de hosts, y sobre todo **el contenedor de nivel 3**,
que está sin empezar. Sin aislamiento, una web remota que expone `run_command`
es una shell remota con pasos extra.

---

## El orden que yo seguiría

El criterio es cuál desbloquea más de los cuatro a la vez, no cuál se parece más
al objetivo.

1. **Backend OpenAI-compatible.** Un solo backend detrás del trait que ya
   existe, y entran LM Studio, `llama-server`, vLLM y los hosts remotos. Toca
   los objetivos 1 y 4 a la vez y es lo más barato de la lista.
2. **Sesiones: SQLite y multi-sesión en `serve`.** Precondición común de
   reanudar en CLI, de la web local usable y de la web multiusuario. Ya está en
   pendientes «bloqueado en nada», y la regla dura ya está fijada: derivable del
   stream de eventos, no una segunda verdad.
3. **Transporte stdio del protocolo.** Desbloquea la extensión de VSCode sin
   escribir todavía la extensión.
4. **REPL/TUI en el CLI.** Lo que más se parece a opencode y lo que menos
   desbloquea al resto — por eso va cuarto, no primero.
5. **Nivel 3 antes que auth**, para el objetivo 4. Auth sobre un `run_command`
   sin aislar no es seguridad, es una puerta con un cartel.

## Dos cosas que salieron al dibujar los Gantt

- **El diagrama de la superficie casi no tiene grados de libertad.** Es una
  cadena: proveedores → persistencia → multi-sesión → stdio → extensión →
  contenedor → auth. El del motor sí los tiene, y la primera semana los decide.
- **La web remota no cabe en el horizonte de cinco meses**, y no por pesimismo
  de estimación: su precondición dura es el contenedor, que está sin empezar, y
  su parte de diseño ni siquiera está escrita.
