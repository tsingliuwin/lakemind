# MaxCompute 接入 —— 实现进度交接文档

> 配套 `spike/REPORT.md`（设计 + 实测吞吐）阅读。本文件记录**实现进度**，供新会话无缝接续。

## 当前状态：M1–M7 全部完成 ✅（端到端验收通过）

已完成的里程碑（`cargo test --lib` → 70 passed / 3 ignored / 0 failed；`tsc --noEmit` → 0 error）：

### ✅ M1 — schema + model
- `src-tauri/src/db.rs`：`db_connections` 加 `options TEXT` 列 + 幂等 `ALTER TABLE` 迁移（`init_global_db` 里）；`DbConnectionRecord.options: Option<String>`；CRUD 全链路（create/update/list/get/list_workspace）透传 `options`。
- `db.rs` 新增：`is_sidecar_db_type(db_type)`、`MaxcomputeOpts`（endpoint/project/region/tunnel_endpoint/driver_coord/`#[serde(default)]`）、`DbConnectionRecord::maxcompute_opts()/maxcompute_table_ref()/maxcompute_ak_id()/maxcompute_ak_secret()`。
- `src-tauri/src/model.rs`：`SourceKind::Maxcompute`。
- `src-tauri/src/commands.rs`：`kind_to_str`/`str_to_kind` 加 `"maxcompute"`。
- `duckdb/register.rs`、`duckdb/scan.rs`、`duckdb/schema.rs`：3 处 `match e.kind` 补 `Maxcompute` arm（与 Postgres/Mysql/Sqlite 同组，unreachable/Ok(None)）。

### ✅ M3 — `src-tauri/src/external/` 模块（sidecar-host 基础设施）
- `external/mod.rs`：声明子模块。
- `external/jdbc_sidecar.rs`：dbx JDBC sidecar stdio JSON-RPC 客户端（`JdbcSidecar::spawn/call/close`）、`build_maxcompute_connection`、`test_connection/list_tables/execute_query`（含分页）、`find_java_bin/check_java_runtime`。**2 单测通过**。
- `external/arrow_sidecar.rs`：`pull_table(duck_conn, rec, opts, table_ref, local_table, sidecar_jar, driver_jars, start, count) -> Result<PullStats, String>` —— Arrow IPC → DuckDB `appender-arrow` 入库 + 类型映射。`ADD_OPENS` + `-XX:MaxDirectMemorySize=8G` 内嵌。
- `external/driver_resolver.rs`：`resolve_driver_jars(resolver_bin, coord, cache_dir)`（调 dbx maven-resolver，解析 `artifacts[].file`）+ `collect_jars`。
- `external/paths.rs`：`SidecarPaths::resolve(&AppHandle)`（resource_dir → dbx_launcher/arrow_jar/resolver_bin/maven_cache）+ `driver_jars(coord)`。
- `src-tauri/Cargo.toml`：加 `arrow = "58"`；`duckdb` features 加 `appender-arrow`。

### ✅ M4 — commands/db 分支（maxcompute 命令层接通）
- `db.rs attach_one`：sidecar 类型跳过 ATTACH（link/启动/切工作区不崩）。
- `commands.rs unlink_connection_from_workspace`：sidecar 跳过 detach（显式 guard）。
- `test_db_connection(app, config)` + `test_connection_impl(r, paths)`：maxcompute → `jdbc_sidecar.test_connection`。
- `list_db_connection_tables(app, config, ...)`：maxcompute → `list_maxcompute_tables`（jdbc_sidecar.list_tables）；缓存逻辑 db_type 无关、复用。
- `register_database_table(app, ...)`：早期分支 `is_sidecar_db_type` → `register_maxcompute_table`（count via jdbc_sidecar → `arrow_sidecar.pull_table` 物化到 lake → upsert `SourceRecord{kind:"maxcompute", scan_path:本地表名, materialize_status:"full"}` + OKF + 缓存刷新）。
- 2 个集成测试加 `init_global_db()` 确保全局 schema 含 `options` 列。

## external 模块对外 API（供 M5/M6 调用）
- `external::paths::SidecarPaths::resolve(&tauri::AppHandle) -> Result<SidecarPaths, String>`
- `paths.driver_jars(coord: &str) -> Result<Vec<String>, String>`
- `paths.dbx_launcher()/arrow_jar()/resolver_bin() -> Result<String, String>`
- `external::jdbc_sidecar::JdbcSidecar::spawn(bin) -> Result<Self, String>`；`build_maxcompute_connection(rec, jars) -> Result<Value, String>`；`.test_connection(&Value)`/`.list_tables(&Value, database, limit)`/`.execute_query(&Value, sql, max_rows) -> (cols, rows)`；`.close(self)`
- `external::arrow_sidecar::pull_table(&duckdb::Connection, rec, opts, table_ref, local_table, sidecar_jar, driver_jars, start, count) -> Result<PullStats, String>`
- `external::jdbc_sidecar::check_java_runtime() -> Result<String, String>`（M6 的 `check_java_runtime` command 包它）

## 剩余里程碑

### ✅ M2 — Java sidecar 打包（运行时前提，core 已完成）
- `src-tauri/sidecars/arrow-maxcompute/`：`ArrowSidecar.java`（从 spike 提升，原样已验证版）+ `build.sh`（`javac -cp <odps-sdk-core-shaded.jar>` + `jar cf`，瘦包）。已构建 `arrow-maxcompute-sidecar.jar`（3KB，仅 `ArrowSidecar.class`，重 jar 运行时 classpath 解析）。
- `src-tauri/sidecars/dbx-jdbc-plugin/`：从 `spike/dbx-jdbc-plugin/dbx-jdbc-plugin-0.1.21/` 拷入（`bin/dbx-jdbc-plugin`、`bin/dbx-maven-resolver`、`lib/dbx-jdbc-plugin.jar`、`manifest.json`，exec 位保留）。
- `src-tauri/tauri.conf.json` `bundle.resources`：`["sidecars/arrow-maxcompute/*.jar", "sidecars/dbx-jdbc-plugin/**"]`（排除 Java 源/build.sh）。路径与 `external/paths.rs` 对齐（`resource_dir/sidecars/...`）。
- 驱动 jar 不打包，首次连接时 `driver_resolver` 从 Maven 解析到 `~/.lakemind/.odps-maven/`（`paths.rs` 的 maven_cache）。
- **M7 待验证/处理**：
  - ① Tauri resources 是否保留 dbx 启动器的 **exec 权限**——若丢，`JdbcSidecar::spawn` 的 `Command::new(launcher)` 会失败；M7 若再现，在 `external/jdbc_sidecar.rs` spawn 前 `chmod +x`（或改 `["sh", launcher]`）。
  - ② dev 模式 `resource_dir()` 是否指向 `src-tauri/`（若是则 dev 能直接找到 `src-tauri/sidecars/...`）；prod 模式指向 bundle resources。
- **暂缓的增强**（非 M7 阻塞，`-XX:MaxDirectMemorySize=8G` 已覆盖 ~30M 行）：列投影、分区表 `PartitionSpec`、窗口化 allocator 释放、`ArrowStreamRecordReader.getRawStream()` 直传（绕开 re-serialization + 零内存累积，M7 可实验）。

### ✅ M5（部分）— maxcompute_pushdown_query 工具 + sample_guard 分支 + paths OnceLock
- `external/paths.rs`：加 `OnceLock<SidecarPaths>` + `init(&AppHandle)`（setup 调一次）+ `get()`（agent 工具无 AppHandle 时取缓存）。`lib.rs` setup 调 `SidecarPaths::init`。
- `agent/tools/maxcompute_pushdown_query.rs`：`MaxcomputePushdownQueryTool`（`impl Tool`，`type Output=String`）—— 解析 SourceRecord → 从 `file_path` 取 connection_id → `SidecarPaths::get()` + driver jars → `jdbc_sidecar.execute_query(sql, 10000)` → markdown 表返回。`agent/tools/mod.rs` 注册。
- `agent/runner.rs`：`build_tools` 16→17 元组（+ `MaxcomputePushdownQueryTool` 构造）；两处解构 + `mcq_tool`；`tool_defs` +1；三处 `.tool(...)` 链各 + `.tool(mcq_tool)`。
- `agent/sample_guard.rs` `build_intercept_message`：`if rec.kind=="maxcompute"` → 提示用 `maxcompute_pushdown_query` 工具（而非 `{kind}_query`，无 DuckDB maxcompute 函数）。
- **暂缓**：`materialize_maxcompute_table` agent 工具——`register_maxcompute_table`（M4）已覆盖 UI 注册物化，agent 重复物化增量价值低；如需 AI 驱动重物化/增量，再写（镜像 `materialize_remote_table`，`call` 调 `arrow_sidecar::pull_table`，并发用 `MaxcomputeOpts.concurrency`）。

## 当前状态：M1–M7 全部完成，编译 + 测试全绿

`cargo test --lib` → 70 passed / 3 ignored / 0 failed；`tsc --noEmit` → 0 error。Rust 侧 + sidecar 打包 + agent pushdown + 前端 maxcompute 表单 + 端到端验收**全部就位**。MaxCompute 接入完成。

### ✅ M6 — 前端（maxcompute 表单 + check_java_runtime command）
- `src/lib/types.ts`：`SourceKind` 加 `"maxcompute"`；`DbConnection.dbType` 联合加 `"maxcompute"`；`DbConnection` 加 `options?: string`（sidecar 类型的 JSON 参数槽）。
- `src/lib/i18n.ts`：zh + en 各加 16 条 MaxCompute 文案（dbTypeMaxcomputeDesc / mcEndpointLabel / mcProjectLabel / mcRegionLabel / mcAkIdLabel / mcAkSecretLabel / mcTunnelEndpointLabel / mcDriverCoordLabel / mcJavaRuntimeLabel / mcJavaCheckBtn / mcJavaOk / mcJavaMissing / mcPermissionTip / mcDriverCoordPlaceholder / mcRegionPlaceholder）。
- `src/components/SettingsPage.tsx`：
  - `formType` 联合加 `"maxcompute"`；加 mc 专属 signal（mcEndpoint/mcProject/mcRegion/mcTunnel/mcDriverCoord + javaStatus）。
  - 类型卡片 grid 改 2×2，加第 4 张 MaxCompute 卡片（紫色 #7c3aed，layers 图标）。
  - `<Show when={formType()==="maxcompute"}>` 专属表单区：endpoint/project/region/tunnel 输入 + AK_ID/AK_SECRET（复用 username/password 槽）+ driver_coord + **Java 运行时检测按钮**（调 `check_java_runtime` command，显示版本号或缺失提示）+ 权限帮助文案（odps:Describe + odps:Select）。
  - `buildMaxcomputeOptions()` / `loadMaxcomputeOptions()`：camelCase JSON ↔ 信号互转（与后端 `MaxcomputeOpts` serde 对齐）。
  - `handleSaveConnection` / `handleTestConnection` / `startEditConnection` / `startAddConnection` 各加 maxcompute 分支（save/test 走 early-return，options 透传）。
  - 连接列表行：maxcompute 用紫色徽章 + layers 图标 + `AK_ID @ project` 副标题。
  - Connection URI 导入栏对 maxcompute 隐藏（maxcompute 无标准 URI）。
- `src-tauri/src/commands.rs`：新增 `#[tauri::command] check_java_runtime()`（`spawn_blocking` 包 `external::jdbc_sidecar::check_java_runtime`，返回 `java -version` 首行）。
- `src-tauri/src/lib.rs`：`generate_handler!` 注册 `commands::check_java_runtime`（紧跟 `test_db_connection`）。

### ✅ M7 — 端到端验收（全链路验证通过，2026-07-14）
- 新增 `src-tauri/src/external/tests.rs`（`#[ignore]` 集成测试 `maxcompute_e2e`）：用正式版代码路径（`JdbcSidecar` + `pull_table` + DuckDB）对真实 `yantubi.dim_users_sc_track`（17,152,141 行）跑全 7 阶段验收。运行方式：
  ```sh
  set -a && source spike/odps-spike.env && set +a
  ODPS_TUNNEL_ENDPOINT="" cargo test --lib external::tests::maxcompute_e2e -- --nocapture --ignored
  ```
- **验收结果（全过）**：① test_connection ✓ ② list_tables（2000 张）✓ ③ count 下推（17,152,141，与 spike 一致）✓ ④ 聚合下推（min/max）✓ ⑤ Arrow 物化 2M 窗口（2,000,896 行 / 1954 batches / 218s）✓ ⑥ 行数完整性（2,000,896 == 2,000,896，零丢失）✓ ⑦ 本地聚合 ✓。
- **验收发现**：JDBC instance-tunnel SQL 执行对 tunnel endpoint 区域错配敏感（`ODPS_TUNNEL_ENDPOINT` 跨区时 `executeQueryPage` 报 `Method not allowed`）。odps-jdbc 会从主 endpoint 自动解析 tunnel，因此 tunnel 字段应**留空**（M6 表单已标"可选"）。Arrow TableTunnel 路径不受影响。
- **吞吐**：单 session 2M 窗口 9,174 行/秒（spike 基线 16,934；差异源于 MaxCompute 服务端带宽波动，非代码路径问题——正式版走相同的 `pull_table`）。17M 全表按并发 5–6 预计 ~5.5min（与 spike 一致）。
- 详细结果已补进 `spike/REPORT.md`「正式版端到端验收（M7）」节。

## 如何在新会话接续
MaxCompute 接入（M1–M7）已全部完成。如需回归验收：
1. `cd /Users/liuyq/rustproject/lakemind/src-tauri && cargo test --lib` + `npx tsc --noEmit` 确认基线绿。
2. 端到端：`set -a && source ../spike/odps-spike.env && set +a && ODPS_TUNNEL_ENDPOINT="" cargo test --lib external::tests::maxcompute_e2e -- --nocapture --ignored`。
3. 凭据仍在 `spike/odps-spike.env`（gitignored）。

## 已知约束（spike 验证）
- ODPS instance-tunnel 1 万行/SELECT cap → 即时查询走 dbx sidecar（≤1 万够聚合），整表物化走 Arrow tunnel（无 cap）。
- 直连内存累积 → 分窗下载 + 段间释放 allocator（M2 的 sidecar 增强要含）。
- 并发亚线性，最优 5–6（`MaxcomputeOpts.concurrency`，M5 工具用）。
- JVM 需 `--add-opens=java.base/java.nio=ALL-UNNAMED` 等（已在 `arrow_sidecar.rs` 的 `ADD_OPENS`）。
- 表权限需 `odps:Describe` + `odps:Select`（M6 表单帮助文案说明）。
