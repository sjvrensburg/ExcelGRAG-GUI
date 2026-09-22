# ExcelGRAG-GUI

**RETIRED --- [ExcelGRAG](https://github.com/sjvrensburg/ExcelGRAG) has its own GUI. Please use that.**

A local web GUI over an [ExcelGRAG](https://github.com/sjvrensburg/ExcelGRAG)
corpus: live, interactive views of the property graphs `eg index` builds, with
the engine's own search and retrieval wired to the canvas.

One process, one browser tab, nothing leaves the machine:

```sh
eg-gui corpus/            # serve the GUI at http://127.0.0.1:8765
eg-gui corpus/ --open     # and open the browser at it
```

## What it does

- **Graph view.** The whole stored graph per workbook, laid out with
  ForceAtlas2 and rendered by Sigma.js (WebGL): workbook, sheets, regions,
  columns, formula groups, defined names, and the edges between them, edge
  thickness by reference weight. The legend is the filter — click a kind to
  hide it, pick a sheet to confine the view to it.
- **Node details.** Click a node for its payload and both directions of every
  edge (what it reads, what reads it, at what weight); click a neighbor to
  walk the graph that way.
- **Search and ask.** `ask` runs the same hybrid search and graph expansion
  `eg ask` does, then lights the answer up on the map: seeds white, contained
  nodes amber, feed/read edges bold, everything else receded — beside the
  cited passage, with every range a live location.
- **Index from the UI.** Point it at a workbook; the same pipeline as
  `eg index` runs in the background (load, build, profile, store, index by
  word and by meaning) and streams its progress to the log panel.
- **Live corpus.** A watcher on the corpus directory pushes changes to every
  open tab over a WebSocket. Index from the UI, from a terminal with
  `eg index`, however — new and re-indexed workbooks appear without a
  reload. Granularity is per-workbook: a graph arrives when it is whole.

## Layout

```
crates/eg-gui     the server: Axum + REST + one WebSocket, the engine's
                  crates linked in-process (eg_mcp::State holds the corpus,
                  the lexical index, and the embedder open between requests)
frontend/         Vite + React + TypeScript + Sigma.js, embedded into the
                  binary at build time and served from the same port
```

The server never shells out to `eg`: it links the same crates the CLI does,
so a corpus built here is the corpus `eg ask` would answer from. The one
deliberate omission is the `check`/`audit` pass `eg index` runs — that is a
developer diagnostic of the lifting code, at a real share of the indexing
time; run `eg check` when you want it.

## Prerequisites

- An [ExcelGRAG](https://github.com/sjvrensburg/ExcelGRAG) checkout as a
  sibling directory (`../ExcelGRAG`). Nothing is on crates.io yet, so the
  crates are path dependencies. The vendored, patched `calamine` it builds
  against is mirrored here as a `[patch.crates-io]` entry; drop both when
  upstream does.
- Rust 1.85+ (as ExcelGRAG), Node 18+ for the frontend build.

## Build and run

```sh
cd frontend && npm install && npm run build && cd ..   # once, and after UI changes
cargo run -p eg-gui -- corpus/ --open
```

For frontend work, run the server as above, then `npm run dev` in `frontend/`
and use the Vite URL: it proxies `/api` and `/ws` to the server, so hot
reload works against the real corpus. A debug build of the server reads
`frontend/dist/` live, so `npm run build && reload` also works without a
cargo rebuild.

Flags: `--port` (default 8765), `--open`, and `--redact-values` — the same
policy `eg serve` has, set once at startup and applied everywhere the GUI
would show a cell.

## API

The REST surface is small and mirrors the MCP tools, which mirror the CLI:

| Endpoint | What it does |
| --- | --- |
| `GET /api/workbooks` | the corpus manifest |
| `GET /api/graph/{hash}` | one stored graph, flattened for rendering |
| `GET /api/graph/{hash}/node/{id}` | a node's payload and neighbors |
| `GET /api/search?q=` | hybrid search hits (word + meaning) |
| `GET /api/ask?q=` | hits, expanded graph context, and the cited passage |
| `POST /api/index` | index a workbook in the background |
| `GET /ws` | `hello`, `corpus` changes, index `log` lines |

A workbook is named by full hash, hash prefix, stored path, or bare
filename, exactly as `eg ask --workbook` resolves them. The server binds to
`127.0.0.1` only, always.

## Status

Early. Cell drill-down (`read_cells`, precedents, dependents, what-if) is the
obvious next surface — the MCP tools exist and the GUI's API can grow to
match them one for one.
