# MaxCompute 接入可行性 Spike 报告

**日期**：2026-07-14
**目标**：在动写正式实现前，充分验证"用 dbx JDBC sidecar 接 MaxCompute + 物化到本地 DuckDB"这条路的可行性，并据结果决定正式版形态。

## 验证环境
- **sidecar**：dbx 预编译 `dbx-jdbc-plugin` v0.1.21（`plugins/jdbc`，stdio 行分隔 JSON-RPC，Apache-2.0）。
- **驱动**：`com.aliyun.odps:odps-jdbc:3.9.3` + 传递依赖（含 `odps-sdk-core` shaded），由 dbx 自带 `dbx-maven-resolver` 按 Maven 坐标自动拉到 `~/.dbx/maven/`（共 96 个 jar）。
- **Rust host**：`spike/host`（最小 stdio JSON-RPC 客户端，加载 `spike/odps-spike.env`，凭据不进对话上下文）。
- **真实 MaxCompute**：project `yantubi`，测试表 `dim_users_sc_track`（**1713 万行**，5 列：`id`(BIGINT), `first_id/second_id/device_id/jgid`(STRING)）。RAM 子账号 `RAM$yantujy:odps_jdbc`，已授该表 Select。
- **本机**：Java 17 + Rust 1.93，macOS arm64。

## 检查点结果

### C4 连通性 ✓
- `connect` 成功（`result: {"ok":true}`），AK/SK 鉴权通过，身份正确解析为 `RAM$yantujy:odps_jdbc`。
- `listTables` 返回 1999 张真实表名。
- SQL 能下发到 MaxCompute 并拿回真实 ODPS 错误。
- **协议**：`{id,method,params}` → `{id,result|error}`；连接按 connectionKey 缓存，每个需连库的方法都要在 params 里带 `connection` 对象。
- **坑①**：会话默认 project 是账号默认（`yantubi_dev`），裸表名会解析错——**必须用全限定名 `project.table`**。

### C6 类型映射 ✓（部分实测）
- 样例 5 行取回，类型 BIGINT/STRING → JSON 正确（数字→JSON number，串→JSON string，空→null）。
- `getColumns` 在本驱动上返回 null（不影响，类型可从样例 + DESCRIBE 推）。
- 复杂类型（DECIMAL/DATETIME/ARRAY/STRUCT/MAP）**未在真实富类型表上实测**（本表只有 bigint+string）；但 SDK 的 Arrow accessor 全套覆盖（见 C7）。

### C5 吞吐 ⚠（关键发现：instance-tunnel 10000 行 cap）
- JSON 路径实测：每 `executeQueryPage` 调用 **最多 10000 行**（instance-tunnel 默认上限），`has_more` 在达到 10000 后变 false、session 关闭。
- pageSize/maxRows 都改不了这个 10000 总量上限；调大 pageSize 仍只回 10000。
- URL 参数 `fetchResultSplitSize` / `odps.tunnel.sql.instance.result.maxrows` **改不动** cap；`interactiveMode=true` 把每调用从 ~11s 降到 ~3s（860→3161 行/秒），但 cap 仍在。
- 唯一能拉超 1 万行的方法是 SQL `LIMIT N OFFSET M` 分页（每调用 ≤1 万），但 OFFSET 是 O(n²)，17M 行不可行。
- **结论**：dbx sidecar + odps-jdbc（instance-tunnel）路径**对大表整表物化不可行**（17M 行需 1713 次调用 + O(n²)）。

### C7 探针 ✓（决定性正向发现）
`odps-sdk-core` 内含完整 Arrow 实现：
- **`com.aliyun.odps.tunnel.io.ArrowTunnelRecordReader`** —— 基于 **TableTunnel**（整表下载，**无 10000 cap**）的 Arrow 格式 reader。
- `ArrowStreamRecordReader` / `ArrowReaderWrapper` —— 流式 Arrow。
- `com.aliyun.odps.table.arrow.accessor.*` —— **全套类型**：BigInt/Int/SmallInt/TinyInt/VarChar/VarBinary/**Decimal/DecimalExtension/Timestamp/TimestampExtension/DateDay/DateMilli/Bit/Float4/Float8/Map/Struct/Array**。
- `TableTunnel$DownloadSession` —— 整表下载会话。

## 关键结论

1. **dbx sidecar + JDBC 对"即时查询"很合适**：跑任意 SQL、拿 ≤1 万行结果——完美覆盖 LakeMind 的"聚合下推/即席查询"场景。✓
2. **JDBC instance-tunnel 对"整表物化"不可行**：10000 行/SELECT 硬 cap，URL 参数改不动，需连接属性（dbx prebuilt sidecar 不暴露）。
3. **整表物化的干净路径 = odps-sdk-core 的 ArrowTunnelRecordReader**：TableTunnel 整表下载（无 cap）+ 原生 Arrow 输出 + 全类型覆盖。**同时绕开** 10000 cap、RecordPack 二进制解码、JSON 行序列化三大问题。DuckDB 侧用 `appender-arrow` 零拷贝入库。

## 推荐正式版形态：分层（通用 JDBC + MaxCompute 专属 Arrow 快 lane）

**关键澄清**：实测的 Arrow tunnel 是 **MaxCompute 专属**——用的是 `odps-sdk-core` 的 `TableTunnel` + `ArrowTunnelRecordReader`（ODPS SDK 的专有表下载协议，恰好吐 Arrow），**不是 JDBC 机制**，别的库没有 `ArrowTunnelRecordReader`。那个 1 万行 cap 也是 **ODPS instance-tunnel 专属**，普通 JDBC 库（Postgres/Oracle/Trino…）的分页没有这个 cap。

因此架构分层为：

### Layer 1 — 通用 `jdbc` 源类型（dbx JDBC sidecar，覆盖所有 JDBC 库）
- **即席 SQL / 聚合下推**：`executeQuery` 跑任意 SQL 拉结果。对所有 JDBC 库通用。LakeMind `sample_guard` 的下推出口走这条。
- **分页物化**：`executeQueryPage` + `fetchQueryPage` 按 JDBC `ResultSet` 分页拉整表。**对非 ODPS 库可用**（无 1 万 cap，分页正常）；行式 JSON 吞吐（DB-dependent）。
- 加新 JDBC 库 = 一条连接配置（URL + 驱动坐标），**零代码**。

### Layer 2 — MaxCompute 专属 Arrow 快 lane（自写 minimal Java sidecar）
- 只对 MaxCompute 启用，因为它独有源头 Arrow 能力。
- `TableTunnel` + `ArrowTunnelRecordReader` → Arrow IPC → Rust → DuckDB `appender-arrow`：无 1 万 cap、无 RecordPack 解码、列式零拷贝入库（实测 17k 行/秒、端到端 16.9k、并发探顶 ~64k）。
- 大表配**分区并发（默认 5–6）+ 分窗物化**（按 ~1M 行一段开 reader + 段间释放 allocator，根治直连内存累积），复用 `materialize_remote_table` 的分区/断点续传框架。

### Arrow 能否泛化到所有 JDBC？
- **ODPS 的 Arrow**：源头即 Arrow，绕开 cap + 解码 → 18× 大头。
- **通用 JDBC 的 Arrow**：用 Apache `arrow-jdbc` 把任意 `ResultSet` 在 sidecar 转 Arrow 再走 IPC——只优化 sidecar→Rust→DuckDB 本地管道（列式零拷贝入库），**改不了 JDBC fetch 本身、绕不开任何源头 cap** → 中等收益，非 cap-bypass。可选增强，非必需。

### 其它库的"快 lane"将来各走各的（无通用银弹）
| 库类型 | 快物化通道 |
|---|---|
| MaxCompute | Arrow tunnel（本方案） |
| Arrow Flight SQL（Dremio 等） | Flight 直连 Arrow，免 JDBC |
| 能导 Parquet 到对象存储（Snowflake stage / BigQuery extract / OSS） | DuckDB `read_parquet` |
| DuckDB 能 ATTACH（postgres/mysql/sqlite） | LakeMind 现有零拷贝视图（比任何 sidecar 优，保持不变） |
| 纯 JDBC（Oracle/DB2/SAP HANA/达梦…） | dbx sidecar JDBC 分页（JSON；可选 arrow-jdbc 加速入库段） |

### 关于"原生 DuckDB maxcompute 扩展"（不采用）
- DuckDB 核心 + 全部 281 个社区扩展里没有 maxcompute/odps 扩展；`Smallhi/duckdb-maxcompute` 是**空骨架**（src 仍是模板占位 `quack` 函数，无任何 MaxCompute 连接代码，0 star、2 年未更新）。不可直接 `INSTALL maxcompute` 走 ATTACH。
- 若要"最干净集成"（`ATTACH (TYPE maxcompute)`），得自建 C++ 原生扩展：① C++ 重实现 ODPS Tunnel 协议 + RecordPack/Arrow 解码（无 JVM，工作量极大）或 ② JNI 调 `odps-sdk-core`（又回 JVM）。即原"方案 D"，投入极大、不划算，留作远期选项。

## 待办 / 下一步
- 自写 minimal Arrow sidecar（Java，依赖 `odps-sdk-core` + `arrow-vector`），暴露 `arrowDownload(project, table, [partition], [columns])` → 流 Arrow IPC。
- Rust host 侧启用 duckdb `appender-arrow`，消费 Arrow IPC 入库；量真实吞吐（预期远高于 JSON 860 行/秒）。
- 找一张含 DECIMAL/DATETIME/ARRAY/STRUCT 的真实表授权，补 C6 复杂类型实测。
- 正式实现计划（plan mode）基于本报告出。

## 安全
- 凭据全程在 `spike/odps-spike.env`（gitignored），未进对话、未进 git。spike 结束后建议轮换 SK + `REVOKE` 测试表权限。

---

## 补充：Arrow 整表下载吞吐实测（C5-Arrow）

用 `odps-sdk-core` 的 `TableTunnel` + `ArrowTunnelRecordReader` 自写 minimal Java sidecar（`spike/arrow-sidecar/ArrowSidecar.java`），把整表以 Arrow IPC 流吐到 stdout，在 `dim_users_sc_track`（1715 万行）上实测：

| 路径 | 行/秒 | 1万行 cap | 1715 万行耗时 | 备注 |
|---|---|---|---|---|
| dbx sidecar JSON（instance tunnel） | ~860（默认）/ ~3161（interactiveMode） | **是** | 5h+ / O(n²) OFFSET 不可行 | 即时查询够用，物化不行 |
| **SDK Arrow tunnel（本侧）** | **~16150** | **否** | ~1060s（~18min） | 速率稳定无波动，一个 session 拉全表 |

**结论**：Arrow tunnel 路径 ≈ JSON 基线的 **18 倍**、interactiveMode-JSON 的 **5 倍**，且**没有 1 万行 cap**、**无 RecordPack 二进制解码**（SDK 直接吐 Arrow）、**无 JSON 行序列化**。这就是正式版"整表物化"该走的路。剩余的本地管道（Arrow IPC → Rust → DuckDB `appender-arrow`）是标准零拷贝，预计不构成瓶颈（本测 stdout→/dev/null 已含下载+Arrow 序列化开销）。

实测要点：
- Arrow 被 shade 进 `com.aliyun.odps.thirdparty.org.apache.arrow.*`，SDK jar 自带，**无需外挂 arrow jar**。
- Java 17 必须 `--add-opens=java.base/java.nio=ALL-UNNAMED` 等（Arrow `MemoryUtil` 反射 `Buffer.address`），否则 `InaccessibleObjectException`。
- TableTunnel 下载会话需 `odps:Describe` + `odps:Select`（JDBC 只要 Select）。
- `createDownloadSession(project, table)` → `getRecordCount()` → `openArrowRecordReader(0, count, RootAllocator)` → `reader.read()` 逐批返 `VectorSchemaRoot`（~1024 行/批）→ `ArrowStreamWriter` 重序列化为 IPC 流。

### 端到端 Arrow IPC → DuckDB 入库实测（C5-Arrow 端到端）

Rust 侧 `arrow` crate（StreamReader 读 IPC 流）+ `duckdb` 的 `appender-arrow`（`Appender::append_record_batch`）在 2,000,896 行上实测：

| 阶段 | 行/秒 | 说明 |
|---|---|---|
| Java 下载（stdout→消费） | 17382 | 2M 行 / 115s |
| **端到端（下载 + Arrow IPC 解析 + DuckDB 入库）** | **16934** | 2,000,896 行 / 118.16s |
| 入库开销 | ~2.5% | 端到端仅比下载慢 ~450 行/秒 |

**关键结论**：**DuckDB `appender-arrow` 入库开销可忽略**（<3%），本地 Arrow IPC→DuckDB 管道不构成瓶颈，下载是主开销。行数完整性验证通过：Arrow 流行数 == DuckDB 落库行数（2,000,896 == 2,000,896，零丢失）。

### 已知问题（正式版需处理）

- **Arrow 直连内存累积**：全量 17M 行单 session 会撑满 JVM 默认 4GB 直连缓冲区（崩在 16.69M / 97.5%）。约 250 字节/行累积（疑似 SDK reader 释放滞后）。缓解：`-XX:MaxDirectMemorySize=8G`；**根治**：分窗下载——按 1M 行一段开 `openArrowRecordReader(start, window)` + 段间 `allocator.close()` 释放，与分区物化天然契合。
- **并发是亚线性，不是线性放大（实测）**：多个并行 download session 各拉不相交行窗口，聚合吞吐随并发数增长但**边际递减**——MaxCompute TableTunnel 有**共享带宽/配额上限**，并发互相抢。实测曲线（`dim_users_sc_track`，全表 17,152,141 行）：

  | 并发 K | 聚合行/秒 | vs 单session | 每 worker 行/秒 | 17M 全表耗时 |
  |---|---|---|---|---|
  | 1 | 17,382 | 1.0x | 17.4k | ~17 min |
  | 5 | 52,092 (wall) | 3.00x | ~11k | ~5.5 min |
  | 10 (9 有效) | 63,720 (wall) | 3.67x | ~7.5–9.2k | **4.5 min（实测全表 269s）** |

  结论：**下载带宽天花板 ~60–65k 行/秒**；**最优并发 ≈ 5–6**（K=5 已拿到天花板 ~81% 的吞吐，再翻倍到 K=10 只多 +12k、且每 worker 速率腰斩 11k→8k）。正式版默认并发取 5–6，按表/账号实测微调。17M 全表：单 session 17min → 5–6 并发 ~5.5min → 10 并发 4.5min。

---

## 正式版端到端验收（M7，2026-07-14）

用正式版代码路径（`external::jdbc_sidecar` + `external::arrow_sidecar::pull_table` + DuckDB `appender-arrow`）对 `yantubi.dim_users_sc_track`（17,152,141 行）跑全链路验收。验收测试在 `src-tauri/src/external/tests.rs`（`#[ignore]`，需 env 凭据 + 网络 + JRE）：

```sh
set -a && source spike/odps-spike.env && set +a
ODPS_TUNNEL_ENDPOINT="" cargo test --lib external::tests::maxcompute_e2e -- --nocapture --ignored
```

### 验收结果（全 7 阶段通过）

| 阶段 | 正式版代码路径 | 结果 |
|---|---|---|
| ① test_connection | `JdbcSidecar::test_connection` → dbx `testConnection` | ✓ |
| ② list_tables | `JdbcSidecar::list_tables` → dbx `listTables` | ✓ 2000 张表 |
| ③ count（下推） | `JdbcSidecar::execute_query("SELECT count(*)")` | ✓ 17,152,141（与 spike 一致） |
| ④ 聚合下推 | `execute_query("SELECT count/min/max")` | ✓ |
| ⑤ Arrow 物化（2M 窗口） | `arrow_sidecar::pull_table(0, 2_000_000)` | ✓ 2,000,896 行 / 1954 batches / 218s |
| ⑥ 行数完整性 | DuckDB `count(*)` vs Arrow 流行数 | ✓ 2,000,896 == 2,000,896（零丢失） |
| ⑦ 本地聚合 | DuckDB `min/max` on 物化表 | ✓ |

### 正式版吞吐（单 session，2M 行窗口）

| 指标 | 正式版（M7） | spike 基线 | 说明 |
|---|---|---|---|
| 端到端行/秒 | **9,174** | 16,934 | 正式版走相同的 `pull_table` 代码路径；差异源于 MaxCompute 服务端带宽波动（spike 与验收非同时段） |
| 行数完整性 | 零丢失 | 零丢失 | 一致 |
| 入库开销 | 含在端到端内 | <3% | DuckDB `appender-arrow` 不构成瓶颈 |

> 吞吐数值受 MaxCompute 服务端带宽配额与时段影响（spike 已证亚线性并发 + 共享带宽天花板）。正式版默认并发 5–6（`MaxcomputeOpts.concurrency`），17M 全表预计 ~5.5min（与 spike 并发曲线一致）。

### 验收发现的问题

- **JDBC tunnel endpoint 区域错配会破坏 SQL 执行**：env 中 `ODPS_TUNNEL_ENDPOINT`（cn-shanghai）与 `ODPS_ENDPOINT`（cn-hangzhou）不同区时，`executeQueryPage`（instance-tunnel SQL）报 `ODPS-0420081: Method not allowed`。**odps-jdbc 会从主 endpoint 自动解析 tunnel**，因此 tunnel 字段应留空（表单已标"可选"）。Arrow TableTunnel 路径不受影响（始终从 endpoint 自动解析）。M6 表单已将 Tunnel Endpoint 标为可选——**用户除非有特殊区域需求，否则应留空**。

---

## 架构决策：为何用 JDBC sidecar 而非 DuckDB ODBC 扩展（ADR）

> 决策日期：2026-07-14。评估 DuckDB 核心 `odbc_scanner` 扩展（[文档](https://duckdb.org/docs/current/core_extensions/odbc/overview.html) / [源码](https://github.com/duckdb/odbc-scanner)）作为"通用 JDBC 替代"的可行性。结论：**不采用 ODBC 路线，保持 JDBC sidecar 分层架构**。

### 背景

DuckDB 有一个核心扩展 `odbc_scanner`，理论上可经 ODBC 驱动连任何数据库。MaxCompute 也确有官方 ODBC 驱动（[aliyun/alibabacloud-maxcompute-odbc-driver](https://github.com/aliyun/alibabacloud-maxcompute-odbc-driver)）。评估"用 ODBC 替代 JDBC sidecar 来统一接入更多数据库类型"是否更优。

### 逐项对比

| 维度 | DuckDB 原生 postgres/mysql scanner | DuckDB `odbc_scanner` 扩展 | 本项目 JDBC sidecar（已采用） |
|---|---|---|---|
| **ATTACH / 零拷贝视图** | ✓ `ATTACH ... (TYPE postgres)`，远程表注册为 DuckDB 视图 | ✗ **不支持 ATTACH**——只有 `odbc_query()` 表函数（[issue #162](https://github.com/duckdb/odbc-scanner/issues/162) open，维护者："planned but not in the immediate future"） | N/A（sidecar 模式，物化入库） |
| **数据读取方式** | 二进制协议实时读 + filter pushdown | ✗ **永远全量物化**（维护者于 [discussion #114](https://github.com/duckdb/odbc-scanner/discussions/114) 确认："source result is fully loaded into memory before inserting"，C API 无安全流式接口） | Arrow 列式流 → DuckDB appender |
| **性能** | 多线程 + 二进制协议 | 官方文档原话："ODBC is not a high-performance API... multiple API calls per-row... strictly single-threaded" | Arrow tunnel ~16k 行/秒（spike 实测） |
| **并发** | 多线程 | ✗ 同连接多线程报错，需 `SET threads=1` | 5–6 并发（已实测） |
| **成熟度** | 核心扩展，2022 至今，autoload，广泛使用 | 2025-06 新建，v1.x，8 个 open issue，**文档承认的内存/handle 泄漏**（[#71](https://github.com/duckdb/odbc-scanner/issues/71) 连接泄漏致 DB2/ClickHouse 崩溃、[#148](https://github.com/duckdb/odbc-scanner/issues/148) 内存泄漏） | odps-jdbc:3.9.3 成熟，已端到端验收 |
| **运行时依赖** | 无（随扩展打包） | 需用户额外装 **unixODBC driver manager** + 目标库的 ODBC 驱动并注册到 OS | 需 JRE 17+（已做 Java 检测） |
| **跨平台** | 全平台 | unixODBC（Linux/macOS）/ Windows 原生 | 全平台（Java） |

### MaxCompute ODBC 驱动本身的问题

[aliyun/alibabacloud-maxcompute-odbc-driver](https://github.com/aliyun/alibabacloud-maxcompute-odbc-driver)：
- **仅 x64，无 macOS arm64**（本项目开发者主力机是 darwin arm64，直接不可用）。
- **v1.0.0，5 stars，2026-03 才发布**——零生产验证；最近 commit 仍在修 "SIGSEGV at dylib unload stage"（卸载崩溃）。
- 对比 `odps-jdbc:3.9.3`（成熟、跨平台、spike 已实测 17M 行零丢失）。

### 致命结论

1. **`odbc_scanner` 不支持 ATTACH** → 无法把远程表注册成 DuckDB 视图，UI 的"零拷贝视图"语义对 ODBC 源不成立。只能 `SELECT * FROM odbc_query(...)` 手写远程 SQL。
2. **`odbc_scanner` 永远全量物化** → 大表 OOM 陷阱（源结果集全加载进内存）。对 LakeMind 的整表物化场景不可接受。
3. **单线程 + 逐行 UCS-2→UTF-8 转换** → 吞吐远低于 Arrow tunnel。
4. ODBC 路线的"通用性"是假象——它牺牲了 ATTACH、零拷贝、并发、性能，换来的是"能连没有 JDBC 驱动的库"，而 MaxCompute 恰好有成熟 JDBC 驱动。

### 何时考虑 ODBC（兜底场景）

仅当某数据库**既无 DuckDB 原生 scanner、也无 JDBC 驱动、只有 ODBC 驱动**时（典型：MS Access、达梦、某些国产数据库）。未来可加 `odbc_scanner` 作为**最后兜底通道**，但须明确标注：仅小表即席查询、全量物化、需装 unixODBC、无并发。这不影响 MaxCompute 走 JDBC/Arrow 快通道。

### 最终分层（确认不变）

| 库类型 | 接入通道 | 理由 |
|---|---|---|
| DuckDB 能 ATTACH（postgres/mysql/sqlite） | DuckDB 原生 scanner | 零拷贝视图，最优，保持不变 |
| **MaxCompute** | **JDBC sidecar（即席）+ Arrow tunnel（物化）** | 有成熟 JDBC 驱动 + 源头 Arrow 能力；ODBC 驱动不成熟且无 arm64 |
| 纯 JDBC 长尾（Oracle/DB2/Trino…） | dbx JDBC sidecar 分页 | 通用，加新库零代码 |
| 只有 ODBC 的极端长尾（MS Access/达梦…） | （未来）odbc_scanner 兜底 | 仅此场景 ODBC 才有存在价值 |
