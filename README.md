# LakeMind

LakeMind is a **local-first lakehouse analysis terminal**. Drop in any folder of
`parquet` / `csv` / `json` / `xlsx` / Delta and it becomes a queryable data lake —
no server, no upload, nothing leaving your machine. A fast, persistent DuckDB
SQL client **and** a conversational Agent that explores the data for you in
plain language — both shipped today.

> **Status — core online, now hardening.** The compute + workspace + task
> foundation and the conversational Agent (streaming LLM ReAct loop) are live
> today; focus has shifted to bug fixing and broadening adoption. The
> file↔table↔task mapping is being hardened.
>
> **Product direction.** Open-source core with a future commercial tier, aimed at
> anyone who analyzes local data files — analysts, engineers, small teams,
> researchers.

## 💡 The Core Philosophy: Why LakeMind?

Most conversational data analysis tools ("chat-with-data" or Text-to-SQL products) obsess over trying to get the LLM to write the 100% perfect SQL query on the first try. They design heavy data catalogs, complex rules, and strict schemas to prevent failures. 

We believe this is a design trap. Real-world business data is volatile, messy, and lacks context. The single-shot accuracy of any LLM has a hard physical limit.

**LakeMind is built on a different paradigm: Exploration over One-Shot Generation.**

*   **10 Trials in 1 Minute > 1 Perfect Try in 2 Minutes**: Spending 2 minutes trying to draft a single "perfect" query is far less productive than spending 1 minute rapidly trying 10 queries based on real-time data feedback and execution error codes.
*   **Active Agentic Action (DDL Materialization)**: Instead of just printing SQL strings and static tables in a chat window, LakeMind’s Agent actively cleans, transforms, and materializes (joins/aggregates) data into persistent physical tables (`t_`) and views (`v_`) locally, allowing immediate analytical reuse.
*   **Zero-ETL Heterogeneous Exploration (Where does the SQL execute?)**: For multi-source data, most products fail to solve where the SQL should actually run. Running on remote databases cannot read local files, while running locally risks network congestion by pulling raw tables. LakeMind solves this by utilizing **local embedded DuckDB as a Central Query Coordinator for "Hybrid Federated Execution"**: local files are computed natively on local CPU cores; large tables on remote databases (PostgreSQL/MySQL) are pre-aggregated on their own servers via query pushdown functions (like `postgres_query`); and the lightweight results are joined locally in client memory with zero ingestion pipelines or middleware (like Trino).
*   **Portable Context & Data Sharing (OKF)**: Sharing raw datasets without business context (join paths, column semantics, metric formulas) leads to "context disconnection." LakeMind packages context into an **Open Knowledge Format (OKF)** bundle (a `.okf/` folder of Markdown and YAML files) that travels natively with the database files. When shared, the receiver's Agent instantly inherits all business memory, preventing LLM cold starts.
*   **Local-First Feedback Loop & Privacy**: While users can directly plug in cloud LLM API keys (like OpenAI, DeepSeek, or Anthropic) to bypass local model deployment friction, LakeMind only sends metadata (table schemas and OKF business definitions) to the cloud for SQL generation. 100% of the raw data rows stay local on your CPU inside the embedded DuckDB database. Executing queries takes milliseconds, making the Agent's self-correction loops instant, cost-free, and private.
*   **Democratizing Cost with Fast, Cheap Models**: Traditional tools rely on expensive, slow frontier models to maximize single-shot query accuracy, making AI-powered data analysis cost-prohibitive for high-frequency work. Because LakeMind leverages instant DuckDB local execution and error feedback, lightweight, blazing-fast, and extremely cheap models (like `deepseek-v4-flash`) perform exceptionally well through self-correction. This slashes token costs to fractions of a cent, making LakeMind a tool that can truly walk into every analyst's daily workflow.

The true revolution of AI in data analysis is not about replacing human developers with single-shot text-to-SQL compilers—it is about **exponentially multiplying the speed and efficiency of data exploration.**

## What works today

### Lake ingest that survives restarts

- Drop a folder or pick a file — `parquet` / `parq` / `csv` / `tsv` / `json` /
  `ndjson` / `xlsx` / `xls` / **Delta** are detected; Hive-style partition dirs
  (`/year=2026/month=06/`) are detected automatically.
- Sources are **materialized into a persistent per-workspace DuckLake**
  (`<workspace>/lake.ducklake` + `lake_data/`) as `s_*` tables/views — they
  survive restarts, no re-scan needed on the next launch.
- **Large** files (default > 100 MB) are registered **in-place** (the source file
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

A **workspace** is an isolated project: its own `lake.ducklake` + `lake_data/`,
its own file directory, its own task list. The left nav groups a workspace's
contents into three kinds:

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

### Conversational Agent

- Streaming ReAct loop (`rig-core`) over **16 tools** — `execute_query`,
  `describe_table`, `list_tables`, DDL (`create_table` / `create_view` /
  `drop_object`), `sample_data`, `materialize_remote_table`, `render_chart`,
  OKF I/O (`load_` / `write_` / `search_okf_recipes` / `tidy_okf_knowledge`),
  tenets (`load_tenets` / `search_tenets`), and `check_source_fingerprint`.
- Providers: OpenAI / Anthropic (streaming) with rate-limit retry + quota
  detection. The Agent actively materializes clean data into `t_` / `v_` tables
  and views for reuse — not just printing SQL. Charts produced mid-conversation
  render inline in the conclusion (switch type, fullscreen, export PNG).

### OKF — Open Knowledge Format

- Context (join paths, column semantics, metric recipes) is packaged into a
  `.okf/` bundle of Markdown + YAML that travels with the data files. A
  receiver's Agent inherits the full business memory — no LLM cold start.

### Federated execution

- Remote PostgreSQL / MySQL tables are queried via native pushdown
  (`postgres_query` / `mysql_query`) so aggregation runs on the source server;
  lightweight results join locally in DuckDB with zero ingestion. Large remote
  tables can be materialized locally in batches via `materialize_remote_table`.

## Stack

| Layer       | Choice                                                          |
|-------------|-----------------------------------------------------------------|
| Shell       | Tauri 2.x (create-tauri-app, Solid + Vite + TS)                |
| Compute     | DuckDB via `duckdb-rs` (`bundled`) — persistent DuckLake (`lake.ducklake` + `lake_data/`) per workspace |
| Metadata    | SQLite via `rusqlite` (`~/.lakemind/lakemind.db`) — workspaces, tasks, sources, config, db_connections, logs |
| Scan        | `walkdir` + Hive partition detection                            |
| Editor      | CodeMirror 6 (`@codemirror/lang-sql`)                           |
| Grid        | `@tanstack/solid-table` + `@tanstack/solid-virtual`            |
| Transport   | JSON over Tauri `invoke` (Arrow zero-copy → M3, future)             |

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
  App.tsx                     # shell: 4-way grid + workspace/task state machine
  components/
    TitleBar · TopBar         # layout toggles, quick actions
    LeftNav.tsx               # workspace tree: 任务 / 文件 / 数据
    DropZone.tsx              # Tauri v2 native drag/drop → import_file_to_workspace
    HomePanel.tsx             # empty-state landing + new chat entry
    SqlEditor.tsx             # CodeMirror 6, Ctrl+Enter, row-cap selector
    ResultTable.tsx           # virtualized rows (TanStack)
    RightInspector.tsx        # column metadata + type-family coloring
    BottomConsole.tsx         # execution log (every query, ok or error)
    ChatView.tsx             # conversational UI (streaming LLM ReAct agent)
    MessageText · ToolSegment · ChartSegment  # assistant / tool / chart cards
    MarkdownRenderer.tsx     # markdown + inline chart-reference rendering
    SettingsPage.tsx          # model / provider / theme / lang / tenets
    Select.tsx                # shared dropdown
  lib/                        # duckdb · types · i18n · theme · chat · chartRef ·
                              # codeConfig · sqlFormat · metrics · logger · updater
src-tauri/src/
  main.rs · lib.rs            # Tauri runtime + command registration (48 commands)
  state.rs                    # AppState: in-memory DuckDB session + source cache
  db.rs                       # SQLite ~/.lakemind/lakemind.db (workspaces, tasks, sources, config, db_connections, logs)
  commands.rs                 # all #[tauri::command] handlers
  model.rs                    # SourceTable / ColumnInfo / SqlResult DTOs (mirror src/lib/types.ts)
  error.rs                    # AppError — preserves raw DuckDB messages
  okf.rs                      # Open Knowledge Format read/write (Markdown + YAML)
  tenets.rs                   # tenets (analyst rules) storage + retrieval
  usage.rs                    # system PREAMBLE + model routing
  fingerprint.rs              # source fingerprint (mtime + size) for change detection
  logging.rs                  # structured logging
  duckdb/
    lake.rs                   # DuckLake: <ws>/lake.ducklake catalog + lake_data/ parquet
    scan.rs                   # filesystem classifier + Hive partition detection
    register.rs               # file → s_* table/view (multi-strategy CSV/Excel loaders)
    schema.rs                 # DESCRIBE + row-count estimation
    execute.rs                # row-capped SELECT → SqlResult (JSON)
    naming.rs                 # s_ / t_ / v_ identifier hygiene + LLM slug
    pathutil.rs               # path hygiene (Win backslash)
  agent/
    runner.rs                 # streaming ReAct multi-turn loop (rig-core)
    llm.rs                    # OpenAI / Anthropic client + connection test
    tools/                    # 16 tools: execute_query, DDL, sample, chart, OKF, tenets, federated
    events.rs · wire.rs       # SSE-style event stream → frontend
    okf_io.rs                 # OKF block I/O for tools
    sample_guard.rs           # intercept aggregations over sampled (partial) tables
    config.rs · error.rs      # agent config + error types
```

### Tauri command surface (48 commands)

| Group | Commands (representative) | Purpose |
|---|---|---|
| **Lake** | `import_file_to_workspace` · `register_workspace_sources` · `list_duckdb_tables` · `list_sources` · `describe_table` · `execute_sql` · `list_tables_fast` · `warmup_sources` · `get_dependencies` · `drop_table_safe` · `delete_file` | file→`s_*` ingest, table listing, DDL, dependency graph |
| **Agent** | `start_agent_chat` · `resolve_tool_confirmation` · `abort_chat` · `test_llm_connection` | streaming ReAct chat + confirm/abort + LLM connection test |
| **DB-Connection** | `get/upsert/delete_db_connection` · `test_db_connection` · `link/unlink_connection_to_workspace` · `list_workspace_connections` · `list_db_connection_tables` · `register_database_table` · `get_table_ddl` | federated PostgreSQL/MySQL: connections + remote catalog |
| **Task** | `load_workspace_tasks` · `save_sql_task` · `save_chat_task` · `delete_task` | sql + chat task persistence |
| **Settings/Config** | `get/set_app_config` · `load/save_settings_json` · `get_system_preamble` · `list_tenets` · `get_tenet_content` | app config, settings JSON, system prompt, tenets |
| **Workspace** | `load_workspaces` · `add_workspace` · `remove_workspace` · `workspace_register_status` | workspace registry |
| **FS** | `select_directory` · `select_file` · `select_files` · `read_directory` | native OS pickers + dir listing |
| **Logs** | `append_log` · `query_logs` · `clear_logs` | execution log store |
| **Misc** | `save_image_from_base64` | high-DPI chart PNG export |

## Data layout on disk

```
~/.lakemind/
  lakemind.db                 # SQLite: workspaces + tasks + sources + config + db_connections + logs
  sqls/<task_id>.sql          # SQL task content
  chats/<task_id>.json        # chat task message history
  <workspace_path>/
    lake.ducklake             # DuckLake catalog (table/view metadata)
    lake_data/                # materialized parquet for s_/t_ tables
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

- **Delta** requires DuckDB's `delta` extension (auto-installed online; offline
  degrades gracefully with a clear error).
- **Result transport is JSON**, so very wide/long results are capped at the row
  limit. Arrow IPC zero-copy is planned for M3.
- **One DuckDB connection per workspace**; concurrent workspaces are not yet
  isolated (M4).

## Roadmap

- **M2** ✓ Shipped — Real Agent: streaming LLM ReAct loop over 16 tools
  (`execute_query` / `describe_table` / `list_tables` + DDL, sampling, remote
  materialization, chart rendering, OKF & tenets I/O); model settings made real;
  file↔table mapping persisted in SQLite (`sources`/`tasks`). (Design notes from
  the predecessor project live in `docs/KNOWLEDGE_DUCKPILOT.md`.)
- **Current focus** — bug fixing and broadening adoption.
- **M3** — Profiler JSON protocol, Polars cleaning pipeline, Analysis Canvas.
- **M4** — Enterprise: project isolation, LanceDB memory, audit logging.

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)
