# Brand marks

The marks the pickers draw beside a provider or a model, one SVG per brand,
named by the slug [lobehub's icons](https://github.com/lobehub/lobe-icons) use
for it, so a pack installed later answers the same names. See
`RECORD/2026-10-26.brand-icons.completed.md`.

From [Simple Icons](https://simpleicons.org) 16.36.0, which publishes its
collection under CC0 1.0. Each mark is the trademark of its owner, shown here
only to say which provider or model a row is; where the owner forbids this use,
it is not here (Apple does not allow its logo to be used this way).

**`minimax.svg` is the one exception to CC0.** It is Simple Icons' MiniMax
mark, which MiniMax gave Simple Icons explicit permission to include
([simple-icons#13982](https://github.com/simple-icons/simple-icons/pull/13982#issuecomment-3531627803))
while saying that this "does not constitute any waiver of MiniMax's rights" and
that its intellectual property terms and brand usage policies "remain fully in
effect". It is here under those terms, not under CC0, and only to say which
provider or model a row is. A pack that has its own `minimax.svg` is drawn
instead, as for every slug.

`docker.svg`, `podman.svg` and `containerd.svg` are the container runtimes'
marks, from the same release: nerdctl is containerd's own client and wears
containerd's. Simple Icons has none for Colima or for Apple's `container`; see
below.

Renamed on the way in: `googlegemini` → `gemini`, `mistralai` → `mistral`,
`zdotai` → `zai` (Z.ai, which makes GLM). `alibabacloud` is Alibaba Cloud's,
worn by its hosts and by Alibaba's Wan until a pack has a mark of Wan's own.

`gemma.svg` is Gemma's mark, a four-pointed star in a frame of corners, as
luu's maintainer supplied it on 2026-10-27; a model of the Gemma family wears
it rather than Google's.

Three are the projects' own rather than Simple Icons', for the engines luu
starts: `ollama.svg` is `docs/ollama-logo.svg` from
[ollama/ollama](https://github.com/ollama/ollama), `llamacpp.svg` is
llama.cpp's mark as its project draws it, and `mlx.svg` is the X of
[mlx-serve](https://github.com/ddalcu/mlx-serve)'s app icon, traced flat
without its tile and its bevel.

And two for container runtimes. `container.svg` is the three switches of
[apple/container](https://github.com/apple/container)'s app icon, traced flat
the same way, each knob a hole; it is the project's icon, not Apple's logo,
which stays out. `colima.svg` is Colima's own favicon (`colima.run`), its paths
unchanged inside an SVG `<mask>` — strokes shown, its white and green fills cut
out — so that drawn as a mask it is the logo's line art and not a silhouette.
