import { useCallback, useEffect, useRef, useState } from "react";

import { connectEvents, getAsk, getGraph, getNodeDetail, getSearch } from "./api";
import { NO_HIGHLIGHT, GraphView } from "./graph/GraphView";
import type { Filters, Highlight } from "./graph/GraphView";
import { EDGE_KINDS, NODE_KINDS } from "./graph/theme";
import { DetailsPanel } from "./components/DetailsPanel";
import { Legend } from "./components/Legend";
import { Sidebar } from "./components/Sidebar";
import type {
  AskResponse,
  GraphDto,
  NodeDetailDto,
  SearchDto,
  WorkbookDto,
  WsEvent,
} from "./types";

export default function App() {
  const [connected, setConnected] = useState(false);
  const [dir, setDir] = useState("");
  const [redactValues, setRedactValues] = useState(false);
  const [workbooks, setWorkbooks] = useState<WorkbookDto[]>([]);
  const [indexing, setIndexing] = useState(false);

  const [graph, setGraph] = useState<GraphDto | null>(null);
  const [graphBusy, setGraphBusy] = useState(false);
  const [detail, setDetail] = useState<NodeDetailDto | null>(null);
  const [focus, setFocus] = useState<{ id: number; nonce: number } | null>(null);
  const focusNonce = useRef(0);

  const [filters, setFilters] = useState<Filters>({
    kinds: new Set(NODE_KINDS),
    edgeKinds: new Set(EDGE_KINDS),
    sheet: null,
  });
  const [highlight, setHighlight] = useState<Highlight>(NO_HIGHLIGHT);

  const [mode, setMode] = useState<"search" | "ask" | null>(null);
  const [queryBusy, setQueryBusy] = useState(false);
  const [search, setSearch] = useState<SearchDto | null>(null);
  const [ask, setAsk] = useState<AskResponse | null>(null);
  const [queryError, setQueryError] = useState<string | null>(null);
  const [logs, setLogs] = useState<string[]>([]);

  const log = useCallback((line: string) => {
    setLogs((previous) => [...previous.slice(-499), line]);
  }, []);

  // --- WebSocket -----------------------------------------------------------
  const onWsEvent = useCallback(
    (event: WsEvent) => {
      switch (event.type) {
        case "hello":
          setDir(event.dir);
          setRedactValues(event.redact_values);
          setWorkbooks(event.workbooks);
          break;
        case "corpus":
          setWorkbooks(event.workbooks);
          {
            const open = currentHash.current;
            if (open && event.removed.includes(open)) {
              setGraphAndFilters(null);
            } else if (open && event.changed.includes(open)) {
              void refreshGraph.current?.(open);
            }
            for (const hash of event.added) {
              log(`workbook added: ${name(event.workbooks, hash)}`);
            }
          }
          break;
        case "log":
          log(event.line);
          break;
        case "index_done":
          setIndexing(false);
          log(
            event.ok
              ? `indexed ${event.path}`
              : `indexing ${event.path} failed: ${event.error ?? "unknown error"}`,
          );
          break;
        case "lagged":
          log(`fell behind (${event.skipped} events skipped); refreshing`);
          if (currentHash.current) {
            setGraphBusy(true);
            void getGraph(currentHash.current)
              .then(setGraphAndFilters)
              .catch(() => setGraph(null))
              .finally(() => setGraphBusy(false));
          }
          break;
      }
    },
    [log],
  );

  useEffect(() => connectEvents(onWsEvent, setConnected), [onWsEvent]);

  // --- Graph loading --------------------------------------------------------
  const currentHash = useRef<string | null>(null);
  const refreshGraph = useRef<((hash: string) => Promise<void>) | null>(null);

  const setGraphAndFilters = useCallback((loaded: GraphDto | null) => {
    setGraph(loaded);
    setDetail(null);
    setHighlight(NO_HIGHLIGHT);
    currentHash.current = loaded?.hash ?? null;
    if (loaded) {
      setFilters({
        kinds: new Set(
          NODE_KINDS.filter((kind) => (loaded.node_kinds[kind] ?? 0) > 0),
        ),
        edgeKinds: new Set(
          EDGE_KINDS.filter((kind) => (loaded.edge_kinds[kind] ?? 0) > 0),
        ),
        sheet: null,
      });
    }
  }, []);

  const openWorkbook = useCallback(
    (hash: string) => {
      if (currentHash.current === hash) return;
      setGraphBusy(true);
      getGraph(hash)
        .then(setGraphAndFilters)
        .catch((e) => log(`could not open the graph: ${message(e)}`))
        .finally(() => setGraphBusy(false));
    },
    [log, setGraphAndFilters],
  );

  refreshGraph.current = async (hash: string) => {
    await getGraph(hash)
      .then(setGraphAndFilters)
      .catch((e) => log(`could not reload the graph: ${message(e)}`));
  };

  // --- Selection ------------------------------------------------------------
  const select = useCallback(
    (id: number) => {
      const hash = currentHash.current;
      if (!hash) return;
      setFocus({ id, nonce: ++focusNonce.current });
      getNodeDetail(hash, id)
        .then(setDetail)
        .catch((e) => log(`could not read the node: ${message(e)}`));
    },
    [log],
  );

  // --- Queries ---------------------------------------------------------------
  const runSearch = useCallback(
    (q: string) => {
      setQueryBusy(true);
      setQueryError(null);
      setMode("search");
      setSearch(null);
      setAsk(null);
      setHighlight(NO_HIGHLIGHT);
      getSearch({ q, workbook: currentHash.current ?? undefined })
        .then((found) => {
          setSearch(found);
          applyHighlightFromSearch(found, currentHash.current, setHighlight);
        })
        .catch((e) => setQueryError(message(e)))
        .finally(() => setQueryBusy(false));
    },
    [openWorkbook],
  );

  const runAsk = useCallback(
    (q: string) => {
      setQueryBusy(true);
      setQueryError(null);
      setMode("ask");
      setSearch(null);
      setAsk(null);
      setHighlight(NO_HIGHLIGHT);
      getAsk({ q, workbook: currentHash.current ?? undefined })
        .then((answer) => {
          setAsk(answer);
          setSearch(answer.search);
          const book =
            answer.workbooks.find((w) => w.hash === currentHash.current) ??
            answer.workbooks[0];
          if (book) {
            const apply = () => {
              const roles = new Map(book.nodes.map((n) => [n.node, n.role]));
              const edges = new Set(
                book.nodes
                  .map((n) => n.edge_kind)
                  .filter((k): k is string => k !== undefined),
              );
              setHighlight({ roles, edgeKinds: edges, workbook: book.hash });
              const seed = book.nodes.find((n) => n.role === "seed");
              if (seed) setFocus({ id: seed.node, nonce: ++focusNonce.current });
            };
            if (book.hash !== currentHash.current) {
              openWorkbook(book.hash);
              // The graph load is async; the highlight applies to it when it
              // lands, via the pending-answer ref.
              pendingAsk.current = apply;
            } else {
              apply();
            }
          }
        })
        .catch((e) => setQueryError(message(e)))
        .finally(() => setQueryBusy(false));
    },
    [openWorkbook],
  );

  const pendingAsk = useRef<(() => void) | null>(null);
  useEffect(() => {
    if (graph && pendingAsk.current) {
      const apply = pendingAsk.current;
      pendingAsk.current = null;
      apply();
    }
  }, [graph]);

  // --- Filters ---------------------------------------------------------------
  const toggleKind = useCallback((kind: string) => {
    setFilters((f) => {
      const kinds = new Set(f.kinds);
      if (kinds.has(kind)) kinds.delete(kind);
      else kinds.add(kind);
      return { ...f, kinds };
    });
  }, []);

  const toggleEdgeKind = useCallback((kind: string) => {
    setFilters((f) => {
      const edgeKinds = new Set(f.edgeKinds);
      if (edgeKinds.has(kind)) edgeKinds.delete(kind);
      else edgeKinds.add(kind);
      return { ...f, edgeKinds };
    });
  }, []);

  const setSheet = useCallback((sheet: string | null) => {
    setFilters((f) => ({ ...f, sheet }));
  }, []);

  const current = workbooks.find((w) => w.hash === currentHash.current) ?? null;

  return (
    <div className="app">
      <Sidebar
        dir={dir}
        workbooks={workbooks}
        currentHash={currentHash.current}
        redactValues={redactValues}
        onOpenWorkbook={openWorkbook}
        onIndexStarted={() => setIndexing(true)}
        onSearch={runSearch}
        onAsk={runAsk}
        busy={queryBusy}
        search={search}
        ask={ask}
        mode={mode}
        onHit={(workbook, node) => {
          if (workbook !== currentHash.current) {
            pendingAsk.current = () => select(node);
            openWorkbook(workbook);
          } else {
            select(node);
          }
        }}
        logs={logs}
        indexing={indexing}
      />

      <main className="main">
        <div className="topbar">
          <div className="topbar-title">
            {current ? fileName(current.path) : graphBusy ? "opening…" : "no workbook open"}
          </div>
          {graph && (
            <div className="topbar-stats">
              {fmt(graph.nodes.length)} nodes · {fmt(graph.edges.length)} edges
              {!graph.formula_groups && " · formula groups on demand"}
            </div>
          )}
          <div className={"connection" + (connected ? "" : " down")}>
            {connected ? "live" : "offline"}
          </div>
        </div>
        <div className="stage">
          {graph ? (
            <>
              <GraphView
                graph={graph}
                filters={filters}
                highlight={highlight}
                selected={detail?.node.id ?? null}
                focus={focus}
                onClear={() => setDetail(null)}
                onSelect={select}
                onReady={() => undefined}
              />
              <Legend
                graph={graph}
                filters={filters}
                onToggleKind={toggleKind}
                onToggleEdgeKind={toggleEdgeKind}
                onSheet={setSheet}
              />
            </>
          ) : (
            <div className="stage-empty">
              {workbooks.length === 0
                ? "The corpus is empty. Index a workbook in the sidebar, or run eg index in a terminal, and it will appear here."
                : "Select a workbook on the left."}
            </div>
          )}
          {queryError && <div className="stage-error">{queryError}</div>}
        </div>
      </main>

      {detail && (
        <DetailsPanel
          detail={detail}
          onSelect={select}
          onClose={() => setDetail(null)}
        />
      )}
    </div>
  );
}

// Highlight from a bare search: every hit on the open workbook gets the seed
// color; hits elsewhere stay in the list for their own click.
function applyHighlightFromSearch(
  found: SearchDto,
  openHash: string | null,
  setHighlight: (h: Highlight) => void,
) {
  if (!openHash) return;
  const roles = new Map(
    found.hits
      .filter((hit) => hit.workbook === openHash)
      .map((hit) => [hit.node, "seed"]),
  );
  setHighlight({ roles, edgeKinds: new Set(), workbook: openHash });
}

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

function fileName(path: string): string {
  const slash = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return slash >= 0 ? path.slice(slash + 1) : path;
}

function name(workbooks: WorkbookDto[], hash: string): string {
  return fileName(workbooks.find((w) => w.hash === hash)?.path ?? hash);
}

function fmt(n: number): string {
  return n.toLocaleString("en-US");
}
