// How each section of the demo is used: per section, a part per component,
// each with a line on what it is for and the file of a parent component that
// uses it — imported in its `:setup`, written as a tag, bound to its own
// state — as it would be pasted. A section with one part opens as a `Modal`, a
// section with several as a `TabModal` with a tab each. See
// `RECORD/2026-10-21.a-design-system-of-components.completed.md`, appended
// 2026-10-22.
//
// `SETUP` is the page's own, in the same shape: what a page loads before any
// section matters.

export const SETUP = { title: "Load the theme", parts: [{
  value: "load", label: "Load it",
  lead: "Two stylesheets and an import map. tokens.css is all a component draws with; luu.css is luu's own surfaces, the file icons' colours and the code colours, and a page that is not luu can leave it out.",
  snippets: [{
    name: "index.html",
    code: `<link rel="stylesheet" href="/theme/default/tokens.css">
<link rel="stylesheet" href="/theme/default/luu.css">

<!-- Before the first module script: the only place a map is read. -->
<script type="importmap">{ "imports": { "@theme/": "/theme/default/" } }</script>
<script type="module">
  import { C79 } from "/vendor/jq79.js"
  C79.fetch("./app.html").mount("#app")
</script>`,
  }],
}] }

const one = (value, label, lead, name, code) => ({ value, label, lead, snippets: [{ name, code }] })

export const USAGE = {
  colours: { title: "Colours", parts: [{
    value: "tokens", label: "Tokens",
    lead: "Every colour is a --luu-* custom property. Change one on :root and every component that reads it follows; a dark island is an attribute.",
    snippets: [
      { name: "brand.css", code: `/* After tokens.css: the accent and the corners, in both tones. */
:root, [data-tone="light"] {
  --luu-accent: #6d28d9;
  --luu-accent-hover: #5b21b6;
  --luu-radius-md: 10px;
}
:root[data-theme="dark"], [data-tone="dark"] {
  --luu-accent: #a78bfa;
}` },
      { name: "panel.html", code: `<!-- One part of a light page dark, with the theme's own dark values. -->
<header class="bar" data-tone="dark">
  <strong>Settings</strong>
</header>

<style scoped>
  .bar {
    padding: var(--luu-space-3) var(--luu-space-5);
    background: var(--luu-surface);
    color: var(--luu-fg);
    border-bottom: var(--luu-border-width) solid var(--luu-border);
  }
</style>` },
    ],
  }] },

  foundations: { title: "Type · Shape · Motion", parts: [one("tokens", "Tokens",
    "Type, space, corners, shadows and motion are tokens too: a component's scoped style reads them, and never a number of its own.",
    "provider-row.html", `<script :setup="{ name = '', facts = '' }"></script>

<div class="row">
  <strong>{{ name }}</strong>
  <span class="facts">{{ facts }}</span>
</div>

<style scoped>
  .row {
    display: flex;
    align-items: center;
    gap: var(--luu-space-3);
    padding: var(--luu-space-2) var(--luu-space-4);
    border-radius: var(--luu-radius-md);
    background: var(--luu-surface);
    box-shadow: var(--luu-shadow-sm);
    font-size: var(--luu-text-md);
    transition: background var(--luu-duration) var(--luu-ease);
  }
  .row:hover { background: var(--luu-hover); }
  .facts { color: var(--luu-fg-muted); font-size: var(--luu-text-sm); }
</style>`)] },

  files: { title: "File icons", parts: [{
    value: "icons", label: "File icons",
    lead: "files.svg is a sprite with no colour of its own: a folder open and shut, and a sheet per kind of file. The --luu-glyph-* tokens in luu.css fill and stroke it.",
    snippets: [
      { name: "tree-row.html", code: `<script :setup="{ name = '', dir = false, kind = '' }"></script>

<li class="row">
  <svg :if="dir" class="glyph folder"><use href="/theme/default/files.svg#i-folder"/></svg>
  <svg :if="!dir" class="glyph" :data-kind="kind || null"><use :href="'/theme/default/files.svg#i-file-' + (kind || 'code')"/></svg>
  <span>{{ name }}</span>
</li>

<style scoped>
  .row { display: flex; align-items: center; gap: var(--luu-space-2); }
  .glyph {
    width: 16px; height: 16px; stroke-width: 1.6;
    fill: var(--luu-glyph-file-fill); stroke: var(--luu-glyph-file-stroke);
  }
  .glyph.folder { fill: var(--luu-glyph-folder-fill); stroke: var(--luu-glyph-folder-stroke); }
  .glyph[data-kind="code"] { --kind: var(--luu-glyph-code); }
  .glyph[data-kind]:not(.folder) {
    stroke: var(--kind);
    fill: color-mix(in srgb, var(--kind) 12%, var(--luu-glyph-file-fill));
  }
</style>` },
    ],
  }] },

  button: { title: "Button", parts: [one("button", "Button",
    "primary is the one thing a view asks for, secondary the rest, ghost what should not compete, danger what loses something — and the heads' tones, dark, accent, ok, warn and bad. click carries the native event in $event.detail.",
    "job-actions.html", `<script :setup="{ job = null }">
  const Button = await import("@theme/components/button.html")
  const { approve, reject } = await import("./store.js")

  let saving = false
  const accept = async () => {
    saving = true
    await approve(job)
    saving = false
  }
</script>

<div class="actions">
  <Button variant="ghost" @click="$emit('later')">Later</Button>
  <Button variant="bad" @click="reject(job)">Reject</Button>
  <Button variant="ok" :loading="saving" @click="accept()">Approve</Button>
  <Button square size="sm" title="More" label="More" @click="$emit('more')">⋯</Button>
</div>

<style scoped>
  .actions { display: flex; justify-content: flex-end; gap: var(--luu-space-2); }
</style>`)] },

  badge: { title: "Badge", parts: [one("badge", "Badge",
    "A short state: tone neutral, accent, ok, warn or bad; dot for a live state, solid for one that should be seen first.",
    "provider-item.html", `<script :setup="{ name = '', running = false, remote = false, isDefault = false }">
  const Badge = await import("@theme/components/badge.html")
</script>

<li class="provider">
  <strong>{{ name }}</strong>
  <Badge :tone="running ? 'ok' : 'neutral'" dot>{{ running ? "running" : "stopped" }}</Badge>
  <Badge :if="remote" tone="warn" size="sm">remote</Badge>
  <Badge :if="isDefault" tone="accent" solid size="sm">default</Badge>
</li>

<style scoped>
  .provider { display: flex; align-items: center; gap: var(--luu-space-2); }
</style>`)] },

  alert: { title: "Alert", parts: [one("alert", "Alert",
    "A message about the view around it. bad is announced as an alert, the rest as a status; dismissible draws a close button that emits dismiss, and the owner decides.",
    "provider-notes.html", `<script :setup="{ host = '', refused = '' }">
  const Alert = await import("@theme/components/alert.html")

  let shown = true
</script>

<Alert :if="shown" tone="warn" title="Not on this machine" dismissible @dismiss="shown = false">
  {{ host }} is on your network, so <code>remote = true</code> is required.
</Alert>
<Alert :if="refused" tone="bad" title="It did not answer">{{ refused }}</Alert>`)] },

  card: { title: "Card", parts: [one("card", "Card",
    "One thing, grouped: a head when there is a title or an actions slot, the content, and a foot.",
    "engine-card.html", `<script :setup="{ engine = {} }">
  const Card = await import("@theme/components/card.html")
  const Badge = await import("@theme/components/badge.html")
  const Button = await import("@theme/components/button.html")
</script>

<Card :title="engine.name" :subtitle="engine.kind + ' · port ' + engine.port">
  <template :slot.actions>
    <Badge :tone="engine.running ? 'ok' : 'neutral'" dot>{{ engine.running ? "running" : "stopped" }}</Badge>
  </template>
  Serves <code>{{ engine.model }}</code>.
  <template :slot.foot>
    <Button size="sm" @click="$emit('stop', engine.name)">Stop</Button>
  </template>
</Card>`)] },

  scrollable: { title: "Scrollable", parts: [one("scrollable", "Scrollable",
    "Something that scrolls on a ground that does not: x, y or both, a max height, a well for a frame, and autoscroll to follow its end.",
    "engine-log.html", `<script :setup="{ lines = [], command = '' }">
  const Scrollable = await import("@theme/components/scrollable.html")
</script>

<!-- A well that follows its end as lines arrive. -->
<Scrollable y frame="well" max="16rem" autoscroll="bottom">
  <pre class="log">{{ lines.join("\\n") }}</pre>
</Scrollable>

<!-- A long command, sideways, on the code's ground. -->
<Scrollable x background="var(--luu-code-ground)">
  <pre class="log">{{ command }}</pre>
</Scrollable>

<style scoped>
  .log { margin: 0; padding: var(--luu-space-3); font-family: var(--luu-font-mono); }
</style>`)] },

  progress: { title: "Progress · Spinner", parts: [
    one("progress", "Progress",
      "How far along: amount out of max, or null for working with no idea how far. tone for a bar that changes meaning as it fills.",
      "model-download.html", `<script :setup="{ model = '', bytes = 0, total = 0, used = 0, window = 8192 }">
  const Progress = await import("@theme/components/progress.html")

  // The window warns past three quarters and refuses past nine tenths.
  $: tone = used / window > 0.9 ? "bad" : used / window > 0.75 ? "warn" : "accent"
</script>

<Progress :amount="total ? bytes : null" :max="total" :label="'Downloading ' + model" />
<Progress :amount="used" :max="window" :tone="tone" label="Window" />`),
    one("spinner", "Spinner",
      "Something is happening and there is nothing to measure it by. It takes the text's colour, so it sits in a line.",
      "asking.html", `<script :setup="{ asking = false }">
  const Spinner = await import("@theme/components/spinner.html")
</script>

<p :if="asking" class="asking"><Spinner size="sm" /> asking the provider…</p>

<style scoped>
  .asking { display: flex; align-items: center; gap: var(--luu-space-2); color: var(--luu-fg-muted); }
</style>`),
  ] },

  kbd: { title: "Kbd", parts: [one("kbd", "Kbd",
    "A key, or a chord: one Kbd per key.",
    "shortcuts.html", `<script :setup>
  const Kbd = await import("@theme/components/kbd.html")
</script>

<p class="hint">
  Search sessions <Kbd>⌘</Kbd><Kbd>K</Kbd> · send <Kbd>Enter</Kbd>
</p>`)] },

  "text-input": { title: "TextInput", parts: [one("text-input", "TextInput",
    "One line of text: :model in and out, prefix and suffix read beside the value, enter with the value in $event.detail.",
    "provider-name.html", `<script :setup="{ taken = [] }">
  const TextInput = await import("@theme/components/text-input.html")

  let name = ""
  let url = ""
  let port = "11434"
  $: clash = taken.includes(name.trim())
  const connect = typed => $emit("connect", typed)
</script>

<TextInput label="Name" :model="name" placeholder="local" :warn="clash" autofocus />
<TextInput label="URL" :model="url" placeholder="http://127.0.0.1:11434" clearable
           @enter="connect($event.detail)" />
<TextInput label="Port" type="number" :model="port" prefix="port" suffix="tcp" size="sm" />`)] },

  textarea: { title: "Textarea", parts: [one("textarea", "Textarea",
    "Several lines: :model in and out, rows to start with, and autosize to grow with what is written.",
    "authority-note.html", `<script :setup>
  const Textarea = await import("@theme/components/textarea.html")

  let note = "Never write outside the working tree."
</script>

<Textarea :model="note" rows="3" autosize placeholder="what a draft is told about itself" />
<p class="count">{{ note.length }} characters</p>

<style scoped>
  .count { margin: 0; color: var(--luu-fg-muted); font-size: var(--luu-text-sm); }
</style>`)] },

  select: { title: "Select", parts: [one("select", "Select",
    "One answer out of a list too long for a segmented choice, as the platform's own select. An option is a string or { value, label, disabled }.",
    "backend-picker.html", `<script :setup>
  const Select = await import("@theme/components/select.html")

  const BACKENDS = [
    { value: "ollama", label: "Ollama" },
    { value: "openai", label: "OpenAI-compatible" },
    { value: "anthropic", label: "Anthropic-compatible" },
  ]
  let backend = "ollama"
  // A new backend makes the last answer about the old one worthless.
  const forget = () => $emit("forget")
</script>

<Select label="Backend" :model="backend" :options="BACKENDS" @change="forget()" />`)] },

  checkbox: { title: "Checkbox", parts: [one("checkbox", "Checkbox",
    "A choice inside a form, taking effect when the form does: :model, and indeterminate for a box over several.",
    "models-shown.html", `<script :setup>
  const Checkbox = await import("@theme/components/checkbox.html")

  let shown = { "qwen2.5-coder:7b": true, "llama3.1:8b": false }
  $: values = Object.values(shown)
  $: all = values.every(Boolean)
  $: some = values.some(Boolean)
  const setAll = on => { shown = Object.fromEntries(Object.keys(shown).map(m => [m, on])) }
</script>

<Checkbox :model="all" :indeterminate="some && !all" label="Every model"
          @change="setAll($event.detail)" />
<Checkbox :model="shown['qwen2.5-coder:7b']" label="qwen2.5-coder:7b" description="4.7 GB · tools" />
<Checkbox :model="shown['llama3.1:8b']" label="llama3.1:8b" description="4.9 GB · no tools" />`)] },

  "radio-group": { title: "RadioGroup", parts: [one("radio-group", "RadioGroup",
    "One answer that needs its reasons beside it: an option is { value, label, description, disabled }.",
    "host-shell.html", `<script :setup>
  const RadioGroup = await import("@theme/components/radio-group.html")

  let host = "loopback"
  const HOSTS = [
    { value: "loopback", label: "loopback", description: "only from this machine" },
    { value: "always", label: "always", description: "whoever holds the token" },
    { value: "never", label: "never", description: "no shell on the host" },
  ]
</script>

<RadioGroup label="Host shell" :model="host" :options="HOSTS" />`)] },

  switch: { title: "Switch", parts: [one("switch", "Switch",
    "A setting that takes effect at once: :model, and change to save it.",
    "sessions-settings.html", `<script :setup="{ autoName = true }">
  const Switch = await import("@theme/components/switch.html")
  const { saveSessions } = await import("./store.js")
</script>

<Switch :model="autoName" label="Name sessions" description="from their first prompt"
        @change="saveSessions({ autoName: $event.detail })" />`)] },

  segmented: { title: "Choice segmented", parts: [one("segmented", "Segmented",
    "Two to five short answers side by side, one radio group to the keyboard. Sized as every other control; block stretches it across the row.",
    "editor-prefs.html", `<script :setup>
  const Segmented = await import("@theme/components/segmented.html")
  const { prefs, setTone, setLayout } = await import("./prefs.js")
</script>

<Segmented label="Editor theme" size="sm" :model="prefs.tone" :options="['light', 'dark']"
           @change="setTone($event.detail)" />
<Segmented label="Columns" block :model="prefs.layout"
           :options="[{ value: 'two', label: 'Two' }, { value: 'three', label: 'Three' }]"
           @change="setLayout($event.detail)" />`)] },

  sizes: { title: "Sizes", parts: [one("sizes", "Sizes",
    "One size for a row: every control takes sm, md or lg and is as tall as --luu-control-sm, md or lg, so a row of them lines up.",
    "filter-bar.html", `<script :setup="{ size = 'sm' }">
  const TextInput = await import("@theme/components/text-input.html")
  const Select = await import("@theme/components/select.html")
  const Segmented = await import("@theme/components/segmented.html")
  const Button = await import("@theme/components/button.html")

  let query = ""
  let backend = "ollama"
  let view = "list"
</script>

<div class="bar">
  <TextInput :size="size" label="Filter" :model="query" placeholder="filter" />
  <Select :size="size" label="Backend" :model="backend" :options="['ollama', 'openai', 'anthropic']" />
  <Segmented :size="size" label="View" :model="view" :options="['list', 'grid']" />
  <Button :size="size" variant="primary" @click="$emit('apply', query)">Apply</Button>
</div>

<style scoped>
  .bar { display: flex; align-items: center; gap: var(--luu-space-2); }
</style>`)] },

  field: { title: "Field", parts: [one("field", "Field",
    "A control and what is said about it: its label, a hint under it, and an error in the hint's place. control is the id of what it labels.",
    "provider-url.html", `<script :setup>
  const Field = await import("@theme/components/field.html")
  const TextInput = await import("@theme/components/text-input.html")

  let url = ""
  $: urlError = url && !/^https?:\\/\\//.test(url) ? "a URL starts with http:// or https://" : ""
</script>

<Field label="URL" control="provider-url" hint="where the server listens" :error="urlError">
  <TextInput id="provider-url" :model="url" :invalid="!!urlError" />
</Field>`)] },

  "input-container": { title: "InputContainer", parts: [one("input-container", "InputContainer",
    "The label inside the box, the description under it: a text input, or type='select' with options. warning says what is wrong in the description's place.",
    "connection.html", `<script :setup="{ reason = '' }">
  const InputContainer = await import("@theme/components/input-container.html")

  let url = ""
  let backend = "ollama"
  const BACKENDS = ["ollama", "openai", "anthropic"]
</script>

<InputContainer label="URL" description="where the server listens"
                :model="url" placeholder="http://127.0.0.1:11434" />
<InputContainer label="Backend" type="select" :options="BACKENDS" :model="backend"
                description="how it is spoken to" :warning="reason" />`)] },

  typeahead: { title: "Typeahead", parts: [one("typeahead", "Typeahead",
    "A text field whose list narrows as it is typed: :model is what is typed, select carries the option chosen. strict refuses anything not listed.",
    "model-field.html", `<script :setup="{ models = [] }">
  const Typeahead = await import("@theme/components/typeahead.html")

  let model = ""
  $: options = models.map(m => ({ value: m.name, label: m.name, description: m.size }))
  const pick = name => $emit("pick", name)
</script>

<Typeahead label="Model" :model="model" :options="options" placeholder="a model name"
           @select="pick($event.detail)" />`)] },

  tabs: { title: "Tabs", parts: [one("tabs", "Tabs",
    "Views of one thing, one at a time. Only the strip is the component's: the owner shows the panel :model names.",
    "turn-panel.html", `<script :setup="{ turn = {} }">
  const Tabs = await import("@theme/components/tabs.html")

  let tab = "context"
  $: items = [
    { value: "context", label: "Context", count: turn.spans?.length },
    { value: "answer", label: "Answer" },
    { value: "calls", label: "Calls", count: turn.calls?.length },
  ]
</script>

<Tabs label="Turn" :model="tab" :items="items" />
<section :if="tab === 'context'">…</section>
<section :if="tab === 'answer'">…</section>
<section :if="tab === 'calls'">…</section>`)] },

  anchors: { title: "Section · AnchorNav", parts: [
    one("section", "Section",
      "A part of a page that can be linked to: an id, a title with its # link, a lead, actions beside the title.",
      "models-section.html", `<script :setup>
  const Section = await import("@theme/components/section.html")
  const Button = await import("@theme/components/button.html")
</script>

<Section id="models" title="Models" divided>
  <template :slot.lead>Where a session sends, and what it may ask.</template>
  <template :slot.actions>
    <Button size="sm" @click="$emit('add')">+ provider</Button>
  </template>
  <ul class="providers">…</ul>
</Section>`),
    one("anchor-nav", "AnchorNav",
      "The sections of a page as links, the one being read marked as it scrolls. offset is the page's sticky head, in pixels.",
      "settings-page.html", `<script :setup>
  const AnchorNav = await import("@theme/components/anchor-nav.html")
  const Section = await import("@theme/components/section.html")

  const SECTIONS = [{ id: "models", label: "Models" }, { id: "engines", label: "Engines" }]
</script>

<div class="page">
  <AnchorNav label="Settings" :items="SECTIONS" offset="64" />
  <main>
    <Section id="models" title="Models">…</Section>
    <Section id="engines" title="Engines">…</Section>
  </main>
</div>

<style scoped>
  .page { display: grid; grid-template-columns: 12rem minmax(0, 1fr); gap: var(--luu-space-6); }
</style>`),
  ] },

  menu: { title: "Menu", parts: [one("menu", "Menu",
    "Actions behind a button, in the top layer, so nothing clips it. An item is { value, label, hint, disabled, danger }, or '-' for a line; select carries the value.",
    "session-row.html", `<script :setup="{ session = {} }">
  const Menu = await import("@theme/components/menu.html")
  const { renameSession, copySession, deleteSession } = await import("./store.js")

  const ITEMS = [
    { value: "rename", label: "Rename", hint: "F2" },
    { value: "copy", label: "Copy as text" },
    "-",
    { value: "delete", label: "Delete", danger: true },
  ]
  const act = what => ({ rename: renameSession, copy: copySession, delete: deleteSession })[what](session.id)
</script>

<li class="session">
  <span>{{ session.name }}</span>
  <Menu size="sm" variant="ghost" label="⋯" title="Session" align="end" :items="ITEMS"
        @select="act($event.detail)" />
</li>`)] },

  tooltip: { title: "Tooltip", parts: [one("tooltip", "Tooltip",
    "A word about what is under the pointer or the focus. The slot is the thing; text is the word.",
    "column-head.html", `<script :setup>
  const Tooltip = await import("@theme/components/tooltip.html")
  const Button = await import("@theme/components/button.html")
</script>

<Tooltip text="Settings" placement="bottom">
  <Button square variant="ghost" label="Settings" @click="$emit('settings')">⚙</Button>
</Tooltip>`)] },

  modals: { title: "Modals", parts: [
    one("modal", "Modal",
      "Open while its owner says so: ESC, the backdrop and the close button only ask. head is auto, integrated, dark or a tone — accent, ok, warn, bad.",
      "delete-session.html", `<script :setup="{ session = {} }">
  const Modal = await import("@theme/components/modal.html")
  const Button = await import("@theme/components/button.html")
  const { deleteSession } = await import("./store.js")

  let asking = false
  const remove = async () => {
    await deleteSession(session.id)
    asking = false
  }
</script>

<Button variant="danger" @click="asking = true">Delete…</Button>

<Modal :open="asking" title="Delete this session?" head="bad" width="26rem" @close="asking = false">
  Its stream goes with it, and nothing brings it back.
  <template :slot.foot>
    <Button variant="ghost" @click="asking = false">Keep it</Button>
    <Button variant="bad" @click="remove()">Delete</Button>
  </template>
</Modal>`),
    one("tab-modal", "TabModal",
      "Several views of one thing in one dialog, its tabs where a title would be. A fixed height keeps it still as the tabs change; the owner shows the panel :model names.",
      "provider-dialog.html", `<script :setup="{ provider = {} }">
  const TabModal = await import("@theme/components/tab-modal.html")
  const Button = await import("@theme/components/button.html")

  let open = false
  let tab = "connection"
  $: TABS = [
    { value: "connection", label: "Connection" },
    { value: "models", label: "Models", count: provider.models?.length, disabled: !provider.models },
  ]
</script>

<Button @click="open = true">Edit {{ provider.name }}…</Button>

<TabModal :open="open" :title="provider.name" head="accent" :tabs="TABS" :model="tab"
          height="24rem" @close="open = false">
  <div :if="tab === 'connection'">…</div>
  <div :if="tab === 'models'">…</div>
  <template :slot.foot>
    <Button variant="ghost" @click="open = false">Cancel</Button>
    <Button variant="primary" @click="$emit('save', provider)">Save</Button>
  </template>
</TabModal>`),
    one("sub-modal", "SubModal",
      "A sheet inside a Modal or a TabModal, out of its parent's head: the parent's head stays lit, ESC closes the sheet alone. tabs as a TabModal's.",
      "workstation.html", `<script :setup>
  const Modal = await import("@theme/components/modal.html")
  const SubModal = await import("@theme/components/sub-modal.html")
  const TextInput = await import("@theme/components/text-input.html")
  const Button = await import("@theme/components/button.html")

  let open = true
  let adding = false
  let model = ""
  let models = []
  const add = () => { models = [...models, model]; model = ""; adding = false }
</script>

<Modal :open="open" title="workstation" width="48rem" @close="open = false">
  <Button @click="adding = true">Add a model…</Button>

  <SubModal :open="adding" title="Add a model" @close="adding = false">
    <TextInput label="Model" :model="model" placeholder="qwen2.5-coder:7b" />
    <template :slot.foot>
      <Button variant="ghost" @click="adding = false">Cancel</Button>
      <Button variant="primary" @click="add()">Add</Button>
    </template>
  </SubModal>
</Modal>`),
  ] },
}
