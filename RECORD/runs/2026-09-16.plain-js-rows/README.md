# The file viewer's rows, with and without the renderer

Evidence for
[`RECORD/2026-09-16.the-viewer-in-plain-js.completed.md`](../../2026-09-16.the-viewer-in-plain-js.completed.md),
which carries the argument and the tables. This directory is what produced them.

## What is here

| | |
| --- | --- |
| `probe.mjs` | the viewer end to end, three arms, against the **debug** binary |
| `e2e-debug.json` | its output |
| `bench.mjs` | the two builders alone, over a payload already in memory |
| `builders.json` | its output, two full runs |
| `release-probe.mjs` | one arm against a **release** binary |
| `release.json` | its output, one binary per UI |
| `verify.mjs` | behaviour rather than time: the two-pass checks, the moved CSS, the other tab kinds |
| `verify.json` | its output |
| `rows.html-arm.js` | the `insertAdjacentHTML` builder, which lost the tie and is not in the tree |

## Running them again

Needs `node`, a Playwright install (`tests/smoke/node_modules` is one), and a
built binary. From this directory, with `node_modules` reachable:

```sh
ln -sfn ../../../tests/smoke/node_modules node_modules

cargo build --bin luu                 # probe.mjs and verify.mjs want debug
node probe.mjs                        # writes the three-arm table to stdout
node bench.mjs
node verify.mjs

cargo build --release --bin luu       # release-probe.mjs wants release
node release-probe.mjs                # or: node release-probe.mjs <binary> <port>
```

`probe.mjs` **edits `crates/luu/ui/` while it runs** — that is how it swaps arms
without rebuilding, since `rust_embed` reads the UI from disk in debug — and
puts the shipped files back in a `finally`. It takes the before arm from
`git show HEAD^:crates/luu/ui/…`, so nothing needs checking out; point it
elsewhere with `BEFORE_REF`. A run that is killed mid-arm leaves the tree on
that arm, and `git checkout -- crates/luu/ui` is the undo.

## Reading the numbers

**Every absolute here is this container's and belongs to nothing else.** It is
about three times slower than the machine phase 8 measured on: its *before*
column is 790 ms on the release binary where phase 8 recorded 271 ms for the
same work. The ratios — a little over 2× on both files, on both builds — are
within-machine and are the part that carries.

`heapMB` is `usedJSHeapSize` and is only worth quoting across an order of
magnitude. The before/after gap is one (hundreds of MB against ~13 MB) and
survives every run. The gap between the two *builders* is not: it swapped
direction between runs, so `builders.json` records it as noise and does not
report it.
