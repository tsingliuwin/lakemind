# ⚓ LakeMind

LakeMind is a **local-first lakehouse analysis terminal**. Drop in any folder of
`parquet` / `csv` / `json` / `xlsx` / Delta and it becomes a queryable data lake —
no server, no upload, nothing leaving your machine. Today it is a fast,
persistent DuckDB SQL client; next, a conversational Agent explores the data for
you in plain language.

> **Status — foundation done, Agent next.** The compute + workspace + task
> foundation is usable today. The conversational Agent (M2) has its full UI but
> currently returns **mock** responses; real LLM integration is the next
> milestone. The file↔table↔task mapping is being hardened.
>
> **Product direction.** Open-source core with a future commercial tier, aimed at
> anyone who analyzes local data files — analysts, engineers, small teams,
> researchers.

## What works today

### Lake ingest that survives restarts

- Drop a folder or pick a file — `parquet` / `parq` / `csv` / `tsv` / `json` /
  `ndjson` / `xlsx` / `xls` / **Delta** are detected; Hive-style partition dirs
  (`/year=2026/month=06/`) are detected automatically.
- Sources are **materialized into a persistent per-workspace DuckDB lake**
  (`<workspace>/lake.duckdb`) as `s_*` tables — they survive restarts, no
  re-scan needed on the next launch.
- **Large** files (default > 200 MB) are registered **in-place** (the source file
  is not copied into the workspace dir); **small** files are copied under the
  workspace so the project is self-contained and portable.
- **Robust multi-strategy loaders** for messy real-world exports:
  - CSV — `sniff_csv` pre-check → full scan → delimiter probing (`;` / `\t` / `|`)
    → GBK-encoding fallback.
  - Excel — 5 header-offset strategies (`A1..A5`) with header-quality scoring and
    an `all_varchar` last resort.
- Delta / Excel extensions are `INSTALL` + `LOAD`ed **lazily**, only when such a
  source actually exists — offline users with no such data are never blocked.

### Workspaces, tasks, files, data (four-layer model)

A **workspace** is an isolated project: its own `lake.duckdb`, its own file
directory, its own task list. The left nav groups a workspace's contents into
three kinds:

- **Tasks** — `sql` queries and `chat` conversations. Persisted (SQLite index +
  content files). `⌘/Ctrl+N` new query, `⌘/Ctrl+Shift+N` new chat, `⌘/Ctrl+S`
  save.
- **Files** — the workspace's on-disk tree; click a data file to import it.
- **Data** — registered `s_*` tables plus any custom tables/views you create with
  SQL, with row counts, kind badges, and partition markers.

### SQL client

- CodeMirror 6 editor (`@codemirror/lang-sql`), `Ctrl/Cmd+Enter` to run.
- Virtualized result grid (TanStack solid-table + solid-virtual); SELECTs are
  row-capped (1K → 1M) to prevent OOM, 100k rows scroll at 60fps.
- Inspector pane with column metadata + type-family coloring; a bottom console
  logs every executed query (success or failure).

## Stack

| Layer       | Choice                                                          |
|-------------|-----------------------------------------------------------------|
| Shell       | Tauri 2.x (create-tauri-app, Solid + Vite + TS)                |
| Compute     | DuckDB via `duckdb-rs` (`bundled`) — persistent `lake.duckdb` per workspace |
| Metadata    | SQLite via `rusqlite` (`~/.lakemind/lakemind.db`) — workspaces + task index |
| Scan        | `walkdir` + Hive partition detection                            |
| Editor      | CodeMirror 6 (`@codemirror/lang-sql`)                           |
| Grid        | `@tanstack/solid-table` + `@tanstack/solid-virtual`            |
| Transport   | JSON over Tauri `invoke` (Arrow zero-copy → M2/M3)             |

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

## Architecture

```
src/                          # SolidJS frontend
  App.tsx                     # shell: 4-way grid + workspace/task/task state machine
  components/
    TitleBar · TopBar         # layout toggles, quick actions
    LeftNav.tsx               # workspace tree: 任务 / 文件 / 数据 (three kinds)
    DropZone.tsx              # Tauri v2 native drag/drop → import_file_to_workspace
    HomePanel.tsx             # empty-state landing + new chat entry
    SqlEditor.tsx             # CodeMirror 6, Ctrl+Enter, row-cap selector
    ResultTable.tsx           # virtualized rows (TanStack)
    RightInspector.tsx        # column metadata + type-family coloring
    BottomConsole.tsx         # execution log (every query, ok or error)
    ChatView.tsx · ChatCard   # conversational UI (currently driven by mock.ts)
    SettingsPage.tsx          # model/theme/lang settings (model panel is placeholder)
  lib/{duckdb,types,i18n,theme,mock}.ts
src-tauri/src/
  main.rs · lib.rs            # Tauri runtime + command registration (16 commands)
  state.rs                    # AppState: per-workspace Arc<Mutex<Connection>> + source registry
  db.rs                       # SQLite: ~/.lakemind/lakemind.db (workspaces + tasks tables)
  commands.rs                 # all #[tauri::command] handlers
  model.rs                    # SourceTable / ColumnInfo / SqlResult DTOs (mirror src/lib/types.ts)
  error.rs                    # AppError — preserves raw DuckDB messages
  duckdb/
    scan.rs                   # filesystem classifier + Hive partition detection
    register.rs               # file → s_* table (multi-strategy CSV/Excel loaders)
    schema.rs                 # DESCRIBE + row-count estimation
    execute.rs                # row-capped SELECT → SqlResult (JSON)
    pathutil.rs               # path/identifier hygiene (Win backslash, s_ prefix)
```

### Tauri command surface (16 commands)

| Group | Command | Purpose |
|---|---|---|
| **Lake** | `import_file_to_workspace` | Copy (small) or in-place register (large), then scan + create `s_*` tables |
| | `register_folder` | Scan a dropped path and register every detected source |
| | `register_workspace_sources` | Switch workspace DB + incremental sync (drop tables for deleted files, reuse existing) |
| | `list_duckdb_tables` | All `main` tables/views (sources + custom), with columns + counts |
| | `list_sources` | The in-memory source registry |
| | `describe_table` | Column metadata for one table |
| | `execute_sql` | Run a row-capped SELECT → `SqlResult` |
| **FS** | `select_directory` | Native OS folder picker (macOS/Windows/Linux) |
| | `read_directory` | List a workspace folder's children |
| **Workspace** | `load_workspaces` · `add_workspace` · `remove_workspace` | Workspace registry (SQLite) |
| **Task** | `load_workspace_tasks` | All tasks of a workspace (sql + chat), with content |
| | `save_sql_task` · `save_chat_task` | Upsert task index + write content file |
| | `delete_task` | Delete task row + its content file |

## Data layout on disk

```
~/.lakemind/
  lakemind.db                 # SQLite: workspaces + tasks index (foreign-key cascades)
  sqls/<task_id>.sql          # SQL task content
  chats/<task_id>.json        # chat task message history
  <workspace_path>/
    lake.duckdb               # the workspace's persistent DuckDB lake
    *.csv / *.parquet / ...   # small imported files (large ones stay in place)
```

## Database Naming Conventions

For data layering and namespace isolation, LakeMind adopts the following naming conventions for DuckDB tables and views:

- **`s_`** (Source): Raw views directly mapped from imported source files (e.g., `s_sales`). These are read-only and may contain headers/comments.
- **`tmp_`** (Temp Table): Intermediate processed physical tables created during data transformation/cleansing (e.g., `tmp_sales_joined`).
- **`tmp_v_`** (Temp View): Intermediate processed virtual views created during data transformation/cleansing (e.g., `tmp_v_sales_filtered`).
- **`t_`** (Target Table): Final clean physical materialized tables, ready for query and analysis (e.g., `t_sales`).
- **`v_`** (Target View): Final clean virtual views, ready for query and analysis (e.g., `v_sales`).

## Tests

```bash
cd src-tauri && cargo test --lib
```

- `execute_select_one` — bundled DuckDB builds and runs SQL
- `execute_enforces_row_cap` — cap + `truncated` flag
- `scan_register_csv_and_query` — CSV scan → table → count
- `scan_register_parquet_and_count` — Parquet + `parquet_metadata` fast path

## Known limitations / active work

- **Chat Agent is mock.** `src/lib/mock.ts` fabricates replies; there is no LLM
  call or Agent loop in the backend yet (M2).
- **File↔table↔task mapping** is resolved at scan time and partly in-memory; a
  persistent mapping table is the next structural improvement (see M2).
- **Delta** requires DuckDB's `delta` extension (auto-installed online; offline
  degrades gracefully with a clear error).
- **Result transport is JSON**, so very wide/long results are capped at the row
  limit. Arrow IPC zero-copy lands in M2/M3.
- **One DuckDB connection per workspace**; concurrent workspaces are not yet
  isolated (M4).

## Roadmap

- **M2** — Real Agent: LLM streaming client + ReAct tool loop over the existing
  `execute_sql` / `describe_table` / `list_duckdb_tables` tools; persistent
  file↔table mapping; model settings made real. (Design notes from the predecessor
  project live in `docs/KNOWLEDGE_DUCKPILOT.md`.)
- **M3** — Profiler JSON protocol, Polars cleaning pipeline, Analysis Canvas.
- **M4** — Enterprise: project isolation, LanceDB memory, audit logging.

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)
