# duckpilot 项目知识沉淀（LakeMind 借鉴资产）

> 来源：`E:\rustproject\duckpilot`（用户前作，2026-05 开源，因 TUI 复杂度停滞于 18 次提交）。
> 整理人：LakeMind 总设计。本文档作为 M2（Agent + WORKSPACE）规划的直接输入。

## 0. 关键事实（移植前必读）

- **代码不可直接复制**：duckpilot 用 `edition = "2024"`，LakeMind 用 `"2021"`；duckdb 版本 duckpilot 是 `1.2`，LakeMind 已升级到 `1.105`。**移植设计模式，重写代码**。
- **语言/序列化模型要改**：duckpilot 把所有查询值拍平成 `String`（丢类型）；LakeMind 已保留 `duckdb::types::Value` → JSON，前端正确渲染。
- **连接模式可复用**：duckpilot 的 `DbEngine { conn: Connection }` + `Arc<Mutex<>>` 与 LakeMind M1 的 `AppState` 一致，验证了此模式正确。
- **API 签名差异**：duckdb 1.105 的 `execute(sql, [])` 需要显式空 params（duckpilot 1.2 也已这样写，兼容）。

---

## 1. duckpilot 是什么（一句话）

基于 Rust + DuckDB 的本地数据分析 Agent：用 DuckLake 把本地 Excel/CSV/Parquet 摄入成表，再通过 OpenAI 兼容 LLM 的 **ReAct 工具调用循环**做自然语言数据探查与业务知识沉淀。

**核心教训**：项目 37% 代码（1323 行）耗在 TUI 上（鼠标/滚动/选择/中文折行/IME），最终停滞。**这正是 LakeMind 选 Tauri + Web 的根本理由——浏览器免费提供终端要手写几千行的东西。**

---

## 2. 对 LakeMind 的价值资产清单（按优先级）

### ★★★ 直接移植到 M2（最高价值）

#### 2.1 Agent 工具层（`agent/tool.rs` + `agent_loop.rs` + `message.rs`，约 740 行）

这是 LakeMind M1 完全没有、且最难自己写对的部分。

| duckpilot 工具 | 位置 | LakeMind M2 对应 |
|---|---|---|
| `list_tables` | `tool.rs:152` | 复用，查 `duckdb_tables()` |
| `describe_table` | `tool.rs:177` | 复用（LakeMind 已有 `describe_table` command） |
| `execute_query` | `tool.rs:214` | 复用 + 加写操作禁令 |
| `sample_data` | `tool.rs:251` | 复用（`SELECT * LIMIT N`） |
| `repair_table_schema` | `tool.rs:285` | **亮点**：让 Agent 自修复数据摄入错误，LakeMind M2 可直接用 |
| `read_business_config` | `tool.rs:316` | M3+ 语义层 |
| `update_business_config` | `tool.rs:332` | M3+ 业务记忆 |

**关键设计模式（M2 直接用）**：
- `ToolRegistry` + `ToolInfo` trait（`tool.rs:15-122`）：统一工具注册 + 生成 OpenAI tools API JSON。
- **写操作安全闸**（`tool.rs:229-235`）：禁 `DROP/DELETE/INSERT/UPDATE/ALTER/ATTACH/DETACH` 前缀——只读 Agent 护栏。
- **表名白名单校验**（`tool.rs:192`）：`chars().all(alphanumeric || '_')`。

#### 2.2 ReAct 循环的工程护栏（`agent_loop.rs`）

| 护栏 | 位置 | 做法 | LakeMind M2 用法 |
|---|---|---|---|
| 步数上限 | `agent_loop.rs:67` | `max_steps = 15` | 直接用 |
| 重复检测 | `agent_loop.rs:131-140` | 工具调用签名去重，重复就注入"别再调工具" | 直接用（防 Agent 死循环） |
| 步数 nudging | `agent_loop.rs:143-147` | ≥6 步提醒收尾 | 直接用 |
| 数据警告注入 | `agent_loop.rs:50-61` | 摄入警告作为 system 消息 | 直接用 |
| **System prompt 数据分层** | `agent_loop.rs:10-33` | `ods_/dwd_/dws_/tmp_` 前缀强制分层 | **核心借鉴**：见下节 |

#### 2.3 数据分层规范（system prompt 的精华，`agent_loop.rs:12-31`）

duckpilot 用 prompt 强制 LLM 自己建分层视图，而非只回答查询。这是比"Text-to-SQL"高一个层次的设计：

- **ODS 层**：原始表自动 `ods_` 前缀（duckpilot 在摄入时加，LakeMind 用 `s_` SOURCE + Agent 建 `dwd_`）
- **DWD 层**：清洗明细视图（脏数据过滤、空值填充）
- **DWS/DM 层**：业务汇总宽表（轻度汇总、多表关联）
- **TMP 层**：会话临时过渡表，结束前 DROP

**对 LakeMind 的映射**：duckpilot 的 `dwd_/dws_/tmp_` 正好对应 LakeMind PRD 的 WORKSPACE 中间表（`t1_/t2_/tN_`）。M2 的 WORKSPACE 引擎应吸收这套分层命名。

#### 2.4 LLM 流式客户端（`llm/mod.rs`，137 行）

手写 SSE，不依赖 SDK。**值得照搬的细节**：
- 支持 DeepSeek/o1 的 `reasoning_content` 字段（`llm/mod.rs:83-87`）——主流 SDK 易漏。
- tool_calls 的 arguments 用**按 index 增量拼接**（`llm/mod.rs:100-128`）——正确处理流式工具调用。
- 三回调：`on_text` / `on_reasoning` / `on_tool_call_started`。

---

### ★★ 改造后复用（M2/M3）

#### 2.5 多策略数据摄入降级链（`engine/mod.rs:109-278`）

duckpilot 最有价值的应用层代码，纯经验积累：

- **CSV 4 级降级**（`engine/mod.rs:204-253`）：`sniff_csv` → `read_csv_auto(ignore_errors)` → 试分隔符 `;/\t/|` → 兜底警告。
- **Excel 5 级 range 偏移**（`engine/mod.rs:156-202`）：处理前几行是标题的导出报表（试 `A1/A2/A3/A4/A5`），最后 `all_varchar=true`。
- **校验启发式**（`engine/mod.rs:280-296`）：DROP → CREATE → `DESCRIBE` 数列数 → **>1 列才算成功**，否则回滚。

**LakeMind 用法**：M1 当前是"单一 CREATE VIEW"，遇到脏 CSV 直接跳过。M2/M3 可引入这套降级链，让摄入更鲁棒。注意 duckpilot 是 `CREATE TABLE`（物化拷贝），LakeMind M1 坚持 `CREATE VIEW`（零拷贝）——降级链的"试错"逻辑可复用，但建表语句保持 VIEW。

#### 2.6 扩展加载的成熟做法（`engine/mod.rs:43-83`）

duckpilot 踩过扩展不可用的坑后的模式：
- `INSTALL` 静默失败（已装则跳过）。
- `LOAD` 才报错。
- `ATTACH ducklake` 失败降级到纯内存，不崩溃。

**LakeMind 已部分采用**（M1 关闸时把 Delta 改成"仅当存在 Delta source 时 LOAD"）。duckpilot 的"DuckLake ATTACH 降级"对 LakeMind 未来引入 DuckLake（M3/M4 持久化湖仓）是直接参考。

#### 2.7 业务记忆三件套（`config/project.rs:33-65`）

`MetricDefinition` / `CleaningRule` / `ViewDefinition` —— 如果 LakeMind 做语义层（M3+），这是现成起点。`update_business_config` 工具展示了 LLM 自己写视图定义并持久化的闭环。

---

### ★ 思路借鉴

#### 2.8 交互模型（TUI 里好的想法，前端重写）

- **四面板 + 视图模式切换**（Chat/Table/Split，`ui.rs`）——SolidJS 组件轻松实现。
- **斜杠命令** `/clear /refresh /chat /table /split /quit`（`app.rs:406-424`）——直接复用。
- **列类型着色**：INT/FLOAT 蓝、TEXT 紫、DATE 橙——LakeMind RightInspector 已实现类似（typeFamily）。
- **状态栏**：DB:✓ LLM:✓ 模型 文件数——BottomConsole 已有雏形。

---

### ✗ 不要移植

- **整个 `tui/` 目录（1323 行）**——停滞根源。chat.rs 手写折行/选择、mouse.rs、input.rs IME 定位，全部由浏览器替代。
- **`build.rs`**——链接无用的 `Rstrtmgr`，删掉。
- **`TableSchema.row_count` 恒 None**——duckpilot 没做行数估算；LakeMind M1 关闸时已用 `parquet_metadata` DISTINCT 公式正确实现，比 duckpilot 强。

---

## 3. duckpilot 的失败教训 → LakeMind 的规避

| duckpilot 踩的坑 | 表现 | LakeMind 如何规避 |
|---|---|---|
| TUI 复杂度吞噬项目 | 6/18 提交修鼠标/滚动/选择 | 选 Tauri + Web，浏览器免费提供这些 |
| NL2SQL 一次性生成 | git 有"重构为agent"提交 | **从 M2 起就用 ReAct 工具循环，不要 NL2SQL** |
| 值类型丢失 | 全部 String 化 | 保留 `Value` → JSON（已做） |
| 行数估算缺失 | row_count 恒 None | parquet metadata DISTINCT 公式（已做） |
| 无图表能力 | TUI 做不到 | SolidJS + ECharts（M3 画布） |

---

## 4. 对 M2 规划的直接输入

duckpilot 的存在让 LakeMind M2（Agent + WORKSPACE）的实现风险大幅降低，因为最难的 3 块都有现成参照：

1. **Agent 工具层**：照 duckpilot `ToolRegistry` + 7 工具模式重写（适配 LakeMind 的 command 架构）。
2. **WORKSPACE 中间表**：照 duckpilot 数据分层规范（`ods_/dwd_/dws_/tmp_`）映射到 LakeMind 的 `t1_/t2_/tN_`。
3. **LLM 流式**：照 duckpilot `llm/mod.rs` 手写 SSE（含 `reasoning_content` + 流式 tool_calls）。

**M2 必须新增（duckpilot 没有）**：
- 多模型后端：duckpilot 只支持 OpenAI 兼容；LakeMind PRD 要求同时支持本地 Ollama（离线）+ 云端。建议用 `rig` 框架统一（PRD 原定）。
- 真正的 ReAct 显式 reasoning：duckpilot 靠 LLM 隐式 ReAct；LakeMind PRD 要求白盒可介入。
- WORKSPACE 三状态机（Active/Persisted/Snapshot）：duckpilot 只有内存表 + config.yaml 持久化，没有快照重放。
