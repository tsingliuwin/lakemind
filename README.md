# ⚓ LakeMind — M1: Pure-Compute Client

LakeMind is a local-first lakehouse analysis terminal. **M1** is the foundation:
a no-AI, no-canvas "plain DuckDB client" that drops the compute base before any
agent features are layered on (see PRD §5.2 gate). Drag a folder in, get a
SOURCE tree, write SQL, browse results in a virtualized grid.

> Scope discipline: M1 contains **no** Rig Agent, WORKSPACE engine, Polars
> pipeline, Profiler, Analysis Canvas, or LanceDB memory. Those land in M2–M4.

## What M1 does

- **Drag-and-drop lake ingest** — drop any folder of `parquet` / `csv` /
  `json` / Delta; Hive-style partition dirs are detected automatically.
- **Zero-copy SOURCE views** — every source is a `CREATE VIEW` over DuckDB's
  `read_*` functions; a 50GB folder costs ~0 bytes until you query it.
- **Fast row counts** — Parquet uses `parquet_metadata()` (row-group footers
  only) so a 50GB folder reports its row count in seconds, not minutes.
- **Hand-written SQL** — a CodeMirror 6 editor with `Ctrl/Cmd+Enter` to run.
- **Virtualized result grid** — TanStack solid-table + solid-virtual; 100k
  rows scroll at 60fps. SELECTs are row-capped by default to prevent OOM.

## Stack

| Layer       | Choice                                              |
|-------------|-----------------------------------------------------|
| Shell       | Tauri 2.x (create-tauri-app, Solid + Vite + TS)     |
| Compute     | DuckDB via `duckdb-rs` with the **`bundled`** feature |
| Scan        | `walkdir`                                            |
| Editor      | CodeMirror 6 (`@codemirror/lang-sql`)               |
| Grid        | `@tanstack/solid-table` + `@tanstack/solid-virtual` |
| Transport   | JSON over Tauri `invoke` (Arrow zero-copy → M2/M3)  |

## Getting started

```bash
npm install --include=dev   # devDeps are needed (vite, tsc, cli)
npm run tauri dev           # first build compiles bundled DuckDB (~5–15 min)
```

> ⚠️ **First build is slow.** `duckdb`'s `bundled` feature compiles DuckDB from
> source via the `cc` crate. Subsequent builds are cached and fast.
>
> ⚠️ The environment's global npm config sets `omit=dev`. Pass `--include=dev`
> (or `npm config delete omit`) or `vite`/`tsc` won't be installed.

### Build the installer

```bash
npm run tauri build         # produces src-tauri/target/release/...
```

## Architecture (M1)

```
src/                          # SolidJS frontend
  components/
    DropZone.tsx              # Tauri v2 native drag/drop → register_folder
    Sidebar.tsx               # SOURCE tree (grouped by parent dir)
    SqlEditor.tsx             # CodeMirror 6, Ctrl+Enter, row-cap selector
    ResultTable.tsx           # virtualized rows (TanStack)
    StatusBar.tsx             # rows / elapsed / truncation / errors
  lib/{duckdb.ts,types.ts}   # invoke wrappers + wire types (mirror model.rs)
src-tauri/src/
  main.rs · lib.rs            # Tauri runtime + command registration
  state.rs                    # AppState: single Arc<Mutex<Connection>> + registry
  commands.rs                 # register_folder / list_sources / describe_table / execute_sql
  model.rs                    # SourceTable / ColumnInfo / SqlResult DTOs
  error.rs                    # AppError — preserves raw DuckDB messages
  duckdb/
    scan.rs                   # filesystem classifier + Hive partition detection
    register.rs               # CREATE VIEW over read_parquet/read_csv_auto/...
    schema.rs                 # DESCRIBE + parquet_metadata fast row count
    execute.rs                # row-capped SELECT → SqlResult (JSON)
    pathutil.rs               # path/identifier hygiene (Win backslash, s_ prefix)
```

### Four Tauri commands (the entire M1 wire surface)

| Command          | Purpose                                            |
|------------------|----------------------------------------------------|
| `register_folder`| Scan a path and `CREATE VIEW` for each SOURCE      |
| `list_sources`   | Return the SOURCE registry                         |
| `describe_table` | Column metadata for one view                       |
| `execute_sql`    | Run a row-capped SELECT and return `SqlResult`     |

## Tests

Backend unit tests cover the full M1 path:

```bash
cd src-tauri && cargo test --lib
```

- `execute_select_one` — bundled DuckDB builds and runs SQL
- `execute_enforces_row_cap` — cap + `truncated` flag
- `scan_register_csv_and_query` — CSV scan → view → count
- `scan_register_parquet_and_count` — Parquet + `parquet_metadata` fast path

## Known limitations (deferred per PRD)

- **Delta** requires DuckDB's `delta` extension (auto-installed online; offline
  falls back with a clear error — M2 hardens this).
- **Result transport is JSON**, so very wide/long results are capped at the
  row limit. Arrow IPC zero-copy lands in M2/M3.
- **Single in-memory workspace**; multi-project isolation is M4.

## Next milestones

- **M2** — WORKSPACE engine + Rig Agent (ReAct, CTAS, Ollama/cloud LLM).
- **M3** — Profiler JSON protocol, Polars cleaning pipeline, Analysis Canvas.
- **M4** — Enterprise: project isolation, LanceDB memory, audit logging.

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)
