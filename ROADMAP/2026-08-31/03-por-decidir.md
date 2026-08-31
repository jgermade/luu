# Lo que queda por decidir

Una decisión es un instante, no una duración: por eso en los Gantt son rombos y
no barras. Aquí están las tres familias que hay abiertas.

## 1. La decisión de rumbo

**Motor o superficie: cuál va primero.**

El repositorio lleva seis días enteros invertidos en el diferenciador —la gestión
de contexto— y en los instrumentos para medirlo. El último recuento del propio
proyecto cierra diciendo *«correr algo»*: la sonda de la puerta, y después la del
mapa. El plano de usuario que hace que esto se use por alguien —CLI conversacional,
extensión, web local usable, web remota— son los pasos 5 y 6 del orden de trabajo,
más uno que no está escrito en ninguna parte.

Las dos vías compiten por la misma persona. **No es una priorización técnica: es
una elección de rumbo, y ninguno de los dos diagramas la puede tomar por ti.**

Lo único que abarata las dos a la vez son las tres piezas que aparecen en ambos
Gantt y se hacen una sola vez: el backend OpenAI-compatible, las sesiones en
SQLite y el contenedor de nivel 3.

## 2. Decisiones ya abiertas en el diseño

Están en la sección *Open questions* de [`loude-design.md`](../../loude-design.md).

| Decisión | Por qué sigue abierta |
| --- | --- |
| **Acotar `network` y `enforcement` con el resto del plan** | Necesita un plan que los declare. Hoy la tarea los hereda del fichero de política |
| **Si `writes` debe acotar también `run_command`** | Un hijo escribe donde las raíces de la tarea permitan; la lista `commands` no dice nada de rutas. Un comando sigue siendo lo más ancho que un plan puede pedir |
| **Quién más puede cerrar una tarea** | Exit codes y tests primero, luego el juez en shadow mode. Hoy el usuario es la única autoridad que cierra |
| **La gramática GBNF concreta** para forzar llamadas válidas con Qwen2.5-Coder, sustituyendo el parseo de texto | Está escrita como pendiente, no como diseño |
| **Si el CLI debe tener puerta** | `luu chat "prompt"` corre un turno con el fichero de política como aprobación permanente, porque un one-shot no tiene bucle humano que interponer. Si debe crecer una, o quedarse la superficie scripted/one-shot que es, está abierto |

## 3. Decisiones que la primera corrida va a forzar

Estas no se pueden razonar desde el sillón: dependen de lo que conteste un
modelo real.

- **¿La gramática adelanta al ranking?** Si un 7B no produce un plan parseable,
  la gramática GBNF deja de ser una línea en la lista de *pequeñas* y pasa a ser
  el siguiente commit, por delante de rankear el mapa. **La sonda de la puerta
  es lo único que puede contestar esto**, y reordena el resto del calendario del
  motor entera.
- **¿Rankear el mapa merece la pena?** Rankear personaliza el mapa por turno y
  por tarea, y un mapa que cambia por turno deja de ser prefijo cacheable. Es un
  intercambio que hay que **medir**, no un parche que aplicar. La sonda del mapa
  es su precondición.
- **¿Un tombstone parcial, o no se poda dentro del turno?** Podar resultados de
  herramientas sería el primer desalojo parcial, y `evicted` deliberadamente no
  sabe describir medio turno. O se diseña ese mensaje, o la poda no entra.

## 4. Decisiones del plano de usuario, que no están en el diseño

Estas son nuevas: el diseño no las contempla, ni a favor ni en contra.

- **La web local: ¿instrumento, producto, o los dos modos?** Hoy los paneles
  pesan más que el chat, y eso fue a propósito: la UI existe para producir
  números. Convertirla en el equivalente del CLI es un reencuadre, no una
  ampliación.
- **Autorización cruzada sandbox × usuario**, para la web remota. Un usuario
  autenticado y un plan aprobado son dos permisos distintos, y nadie ha escrito
  cómo se componen. **`loude-design.md` no menciona auth, ni usuarios, ni
  multi-host**; lo único que hay es una línea en deudas de seguridad diciendo
  *«auth, si `serve` se expone fuera de loopback»*.
- **Remotizar otros hosts** (lo que hace LM Studio): qué se registra, quién
  enruta una sesión a qué host, y qué parte del sandbox viaja con ella. Sin
  diseñar.
- **MCP y subagentes**: ni construido, ni decidido, ni en el diseño.

## Un principio del repositorio que aplica a casi todas

De `AGENTS.md`, y conviene tenerlo delante al decidir cualquiera de las de
arriba:

> Las estrategias de contexto se miden, no se argumentan. Un cambio en la
> compactación, la selección por relevancia o el reparto del presupuesto
> necesita números del mismo modelo y el mismo conjunto de tareas, antes y
> después. **«Menos tokens» no es un resultado por sí solo: tirar el fragmento
> que el modelo necesitaba también es menos tokens.**
