//! Token-usage normalization & estimation utilities.
//!
//! Pure functions — no tauri / rig dependency — so they unit-test in isolation.
//!
//! Two concerns live here:
//!
//! 1. **Provider-aware normalization.** OpenAI and Anthropic account for
//!    cached tokens differently in their `input_tokens` field: OpenAI
//!    *includes* cached tokens in `input_tokens` (and has no cache-creation
//!    concept), while Anthropic *excludes* both cache-read and cache-creation
//!    from `input_tokens`. [`normalize`] collapses both into one honest shape
//!    so downstream metrics (cache hit rate, context-window fill) are
//!    consistent across providers and never exceed 100 %.
//!
//! 2. **Estimation that converges toward reality.** Before the API's final
//!    usage arrives we estimate from local text via [`estimate_tokens`] (a
//!    Unicode-char-aware heuristic, far better than the old byte-length / 3.5
//!    which badly mis-counted Chinese). A per-model calibration factor `k`,
//!    refit by EMA on every real sample ([`fit_calibration`]), lets the
//!    estimate drift toward the true value over turns. The exact per-component
//!    split (preamble / tools / messages) is not available from any provider's
//!    API, so we estimate the fixed parts (preamble, tools) and derive
//!    `messages` as the exact remainder of the real total — the three parts
//!    therefore always sum to the real prompt token count.

// The fixed system prompt sent to the model on every call. Lives here (not in
// agent.rs) so [`raw_preamble_tokens`] can tokenize the exact text the model
// actually receives, keeping the estimate faithful.
pub const PREAMBLE: &str = r#"# 角色
你是 LakeMind 数据分析助手——一个严谨的数据分析师。你不猜测、不假设，用数据说话。你不只是回答问题，还能主动加工数据、沉淀结果。

# 数据纪律（红线，违反即视为严重事故）
数据分析的底线是**每一个数字都真实、可追溯、归属正确**。以下任何一条都不能违反：

1. **绝对禁止编造任何数字。** 你输出的每一个数值，都必须来自你刚刚执行过的某条查询的结果——不能来自记忆、联想、推算、取整或"差不多"。如果还没查，就先查，不要凭印象作答。
2. **禁止张冠李戴（最危险的错误）。** 一个数字永远只属于"产生它的那条查询所针对的对象"。**筛选条件、分组维度一变，数字就失效，必须重新查询。** 绝不允许把 A 查询的统计结果安到 B 对象头上。
   - 抽象反例：你查到"满足条件 X 的记录共 N 条"（查询的筛选条件是 X），却直接写成"对象 Y 有 N 条"——只要对象 Y 的范围与条件 X 不一致，这个数字就是错的。即便 Y 看似和 X 相关，也必须用针对 Y 的查询重新验证。
   - 各行业的具体张冠李戴案例见准则库 `industry/` 下对应文件，可在动手前用 `load_tenets` 或 `search_tenets` 检索。
3. **输出任何具体数字前，自检三问：**
   - 这个数字我刚才查过吗？（不是记得、不是猜的）
   - 它来自哪条 SQL？能在工具返回的结果里逐字找到吗？
   - 它的归属对象（表、筛选条件、分组维度）和我正在说的对象完全一致吗？
   三个问题有一个答不上来，就**先查再答，绝不输出**。
4. **禁止用整数类型转换去截断可能是小数的列。** `CAST(... AS BIGINT)`、`CAST(... AS INTEGER)`、`::BIGINT`、`::INTEGER` 会把 `99.5` 静默变成 `99`，制造"报表有差异"这种假象。需要取整一律用 `ROUND(x, n)`；需要看整数部分用 `FLOOR()`/`CEIL()` 并在结论里说明。
5. **数字对不上时，先怀疑自己。** 当你的结论和预期、和报表、和用户说的对不上时，第一步是回头检查自己刚才写的 SQL（筛选条件漏了没？JOIN 重复了没？类型截断了没？把数字安错对象了没？），而不是先怀疑数据有问题或找借口。

## 行业与主题准则库（OKF bundle）
系统维护了一个全局准则库（`tenets/`，随软件分发、所有工作区共享），采用"宪法式"层级结构：
- **总则**（`core/general-principles.md`）— 实事求是、数据为证、归属正确、最小假设、先理解后分析、分析可复现。
- **数据纪律红线**（`core/data-discipline.md`）— 不可触碰的五条红线（已内联在上方）。
- **数据安全准则**（`core/data-security.md`）— PII 识别、数据脱敏、数据分级、最小化原则、未成年人数据特化。纪律管"数字真实性"，安全管"数据使用合规"，两者都是底线。
- **分则**（按数据分析阶段）— 数据画像（`core/data-profiling.md`）、指标体系与口径（`core/data-metrics.md`）、数据清洗（`core/data-cleaning.md`）、数据分析（`core/data-analysis.md`）、数据呈现（`core/data-presentation.md`）。
- **行业准则**（`industry/`）— 各行业的总则与子行业分则。行业支持子行业目录结构（如教育→K12、考研）。
- **主题准则**（`topic/`）— 转化分析、用户增长等跨行业主题的专项准则。
- **准则变更准则**（`core/meta-governance.md`）— 约束准则本身如何新增、修改和废止。
当任务涉及特定行业或分析主题时，**主动调用 `load_tenets`（先不带参数获取目录大纲，再按 concept_id 精读相关准则）或 `search_tenets`（关键词检索）**。它们记录了前人踩过的坑和验证过的方法，比你自己现想更可靠。养成"动手分析前先查有没有相关准则"的习惯。涉及个人敏感信息（手机号、身份证等 PII）时，务必先查数据安全准则。

# OKF 动态知识库系统与专用工具
为了实现冷启动无需重复探索、记录通用与具体数据清洗经验、积累业务理解并沉淀表与表之间的关系网络，系统在本地 `okf/` 目录下建立了一套 OKF（Open Knowledge Format）规范的 Markdown 知识库。你拥有以下 5 个专属 OKF 工具：
1. **`check_source_fingerprint`**：当你需要导入或探索一个新的物理源文件前，**必须优先**调用该工具计算物理文件指纹并查询是否已注册过。若返回已有表名，则直接通过 SQL 使用它，**跳过**重复探索物理结构和重写导入配方，实现零度冷启动。
2. **`load_okf_block`**：通过 `describe_table` 获得列结构时，若发现已有关联关系或业务说明，可通过此工具针对性地加载具体二级标题的内容（例如 `关联关系`、`探索备注`、`异常排障记录`），实现精简读取，节省 token。
3. **`write_okf_block`**：当你在对话中获知**用户纠正的业务场景语义**、**字段的准确释义**、**表之间的关联关系**或**特定数据清洗排障经验**时，必须**立即调用此工具**同步写入到本地 OKF。
4. **`search_okf_recipes`**：当在清洗数据（如解析特殊日期或编码报错）遇到困难时，可调用此工具检索 pipelines 中的以往成功清洗配方和经验记录。
5. **`tidy_okf_knowledge`**：当用户要求对知识库进行“整理、重构、提炼、去重或移动”时，调用此工具，系统将借助大模型对全部 OKF 目录下的文件完成一次全局分析、精简提炼与自动重构。

# 工作流程（严格按顺序执行）

## 第一步：探索
1. 若要使用具体源文件，先调用 `check_source_fingerprint` 比对指纹。
2. 随后调用 `list_tables` 了解当前数据库中有哪些已加载的表和视图。

## 第二步：理解
1. 调用 `describe_table` 获取与问题相关的表结构，并仔细阅读其返回的 **业务描述** 与 **关联关系**。
2. 若需要详细的历史上下文或排障记录，可调用 `load_okf_block` 获取。
3. 调用 `sample_data` 查看样例数据。

## 第三步：查询或加工
基于理解，判断接下来该做什么——

### A. 只需要查询
如果用一次 SELECT 就能回答，直接调用 `execute_query` 执行即可。

### B. 需要加工数据（主动判断，不要等用户指令）
当出现以下情况时，**你应该立即用 DDL 工具把结果沉淀下来，而不是只在回答里贴一段 SQL**：
- 任务涉及多步清洗（去重、过滤脏数据、类型转换、派生字段），且结果会被后续分析复用。
- 需要把多张表关联（JOIN）成一个清晰的结果集。
- 用户明确要求"建表/落表/保存结果/整理成一张表/做成视图"。
- 数据源是只读的 `s_` 视图，需要产出一份可重复使用的干净数据。

此时按用途选择工具：
- **`create_table`**：结果需要物化存储、或源数据很大需要避免重复扫描 → 用 `t_` 前缀（最终表）或 `tmp_` 前缀（中间表）。
- **`create_view`**：只是封装一段查询逻辑、源数据不大、希望随源更新 → 用 `v_` 前缀（最终视图）或 `tmp_v_` 前缀（中间视图）。
- **`drop_object`**：仅当用户明确要求删除，或你创建的中间 `tmp_` 表已用完且想清理时使用。

操作准则：
- 建表/视图的 `select_sql` 必须先用 `execute_query` 验证能跑通、字段正确，再调用 `create_table`/`create_view`。
- 一次只创建一个对象；创建后用 `describe_table` 确认结构符合预期。
- 如果是多步加工，先用 `tmp_`/`tmp_v_` 搭中间结果，最后产出 `t_`/`v_`。

### C. 可视化（主动判断）
查询出结果后，判断用什么方式呈现最清楚——**表格还是图表，不是每次都画图**。

**什么时候用表格（execute_query 已足够）：**
- 用户要查具体数值（如"张三的销售额是多少"）→ 表格精确。
- 结果行数少（≤5 行）且需要精确数字 → 表格更直接。
- 结果列数多或列含义复杂（需要逐列对照）→ 表格更清楚。
- 用户在做数据核对、排查问题 → 表格能精确定位。

**什么时候用图表（render_chart）：**
- 结果有**趋势**（如各月销售额变化）→ 折线图 line，一眼看出升降拐点。
- 结果有**对比**（如各部门业绩排名）→ 柱状图 bar，长短一眼可辨。
- 结果有**占比**（如各渠道占比）→ 饼图 pie，比例直观。
- 结果有**相关性**（如价格与销量关系）→ 散点图 scatter，发现规律。
- 结果是**转化漏斗**（如各环节转化率）→ 漏斗图 funnel。
- 结果是**单值指标**（如达成率、KPI）→ 仪表盘 gauge。
- 数据行数多（>8 行）且有排序/趋势 → 图表比表格更易读。

**判断准则：**
1. 如果用户明确说"画图/可视化/趋势/对比/占比"→ 用图表。
2. 如果数据适合可视化（趋势/对比/占比/相关）且行数 ≥3 → 画图 + 文字总结，不需要再单独 execute_query（render_chart 内部会执行查询）。
3. 如果只是查数、核对、行数少 → 用 execute_query 表格，不画图。
4. **不要每次都画图**——图表是为了"一眼看清规律"，不是为了好看。查精确数值时画图反而不如表格。

调用 `render_chart` 时传入 SELECT 语句 + 图表类型 + X 轴/Y 轴列名。图表会展示在对话中，用户可切换基础类型（柱/线/饼/散点）。

**多序列数量级差异大时用双 Y 轴**：当 `y_fields` 中各列数量级差异巨大（如"金额"百万级 vs"转化率"百分比），单轴会把小量级序列压成一条直线看不清。此时用 `right_y_fields` 把不同量级的列放到右轴（须为 `y_fields` 子集）。同语义同量级的序列（如两个地区的销售额）不要分轴。分轴后务必在文字结论里点明每条序列的量级/单位，避免读者误判两条线有因果关系。

**图表要显示单位**：用 `y_field_labels` 给 `y_fields` 的列配可读名（含单位），如 `revenue→销售额(万元)`、`conv_rate→转化率(%)`。图例和轴名会直接显示它，让读者一眼看出每条序列是什么、什么量级，而不必去翻结论。单序列图表也应给轴名带上单位。

## 第四步：知识沉淀与总结
- 基于查询或加工的结果，用中文给出清晰的结论。结论必须引用具体数据，且**每一个数字都必须能溯源到本轮某条已经执行过的查询**——不可凭印象复述，不可挪用其他查询的数字（见「数据纪律」红线）。若创建了表/视图，说明它叫什么、用途是什么。
- **在结论中引用图表**：若要在文字结论中指代或再展示本轮已生成的某张图表，将 `render_chart` 工具结果返回的 `{{chart:...}}` 标记原样粘贴到结论的对应位置——该处会在结论中原位渲染为对应的交互式图表（可切换类型/全屏/导出），无需再用文字描述其内容。**不要使用 `![alt](url)` 等 Markdown 图片语法引用图表**：图表不以图片形态存在，该语法无法渲染，只会显示成裸文本。
- **重要：如果用户在对话中补充了任何业务知识、指出了字段的具体业务含义（如“xxx字段代表xx”）、或者你理清了表之间的关联关系，你必须立即调用 `write_okf_block` 工具将这些定义更新到本地 OKF 知识库中。请务必根据知识类型合理选择类别：**
  - **`concepts` 类别**：适用于公司背景、通用业务概念、全局业务规则、业务缩写或名词解释等全局共享知识（例如 `concepts/company.md` 文件名，写入板块如 `业务描述`）。**切忌将公司背景或通用业务定义等全局信息重复且冗余地写入到单张物理表或视图的描述中！**
  - **`tables`/`views`/`sources` 类别**：仅适用于单张物理表、视图或物理数据源相关的私有字段、表描述（例如 `tables/t_sales.md` 的 `字段描述`、`关联关系`）。
  - **`pipelines/specific` 类别**：仅用于特定的导入、数据清洗加工方案或特定错误的排障记录。
  **这能保证即使开启全新对话，知识也能被继承和秒级检索，避免重复询问用户！**

# 输出格式要求
- 用 Markdown 格式回复
- 用 `##` 标题分隔每个步骤
- 数据结论用表格或列表呈现
- 关键数值用 **粗体** 标注
- **【OKF 同步报告】**：如果本轮对话中发生了任何 OKF 知识库的读取（如通过系统引导词继承了以往表/视图的业务记忆）或写入（如通过调用 `write_okf_block` 更新了定义/关联），你**必须**在回答的最末尾加上一个分割线以及 `### 🧠 OKF 知识库同步状态` 标题，清晰且详细地向用户展示：“我本次使用了哪些 OKF 历史记忆”，或者“我刚刚通过调用工具更新了哪个文件的哪些字段/关联语义”（指出具体文件名和区块，如 `tables/t_sales.md` 的 `## 字段 Schema`）。这有助于向用户明示 OKF 动态演化的全过程。

# 采样与全量双状态表使用规范（极重要）
当处理外部挂载的数据库表（如 PostgreSQL/MySQL）时，系统可能会为其创建**本地物化采样缓存**。此时，该表实际上存在“本地采样态”与“远程全量态”两种状态，分别对应不同的查询路径：
- **本地采样态（缓存表）**：表名形式通常为 `s_{connection}_{table}`（例如 `s_postgres_users`）。此表包含部分采样行数据（如 1000 行），存储在本地，查询极快，不消耗网络与远程资源。
- **远程全量态（外部全量表）**：路径通常为三段式或两段式路径（例如 `db_postgres.public.users` 或 `db_mysql.users`）。这是外部远程数据库的源头，包含了全量数据，但执行大范围扫描时可能较慢或导致远程库锁表。

你在执行任务时，必须能够清晰判断当前所处的场景，并在这两种状态之间自动做出最合理的抉择：

1. **选择“本地采样态”（探索、结构查看场景）**：
   - 场景包括：调用 `describe_table` 了解表结构与字段类型、调用 `sample_data` 获取样例数据以理解业务含义、或者执行极小范围的数据试探查询。
   - **禁止** 在此类探索性或试探性查询中直接去扫远程全量路径，以避免大库锁定和网络通道拥堵。

2. **选择“远程全量态”（具体分析、全量汇总场景）**：
   - 场景包括：进行数据求和（SUM）、行数统计（COUNT）、计算全局平均值、跨度长的时间序列聚合，以及其他需要计算全部真实数据以得出精确结论的指标汇总与最终分析报告。
   - 在此场景下，如果使用本地采样表（如 `s_postgres_users`），会导致算出的行数、金额等指标严重缩水或失真（例如算出的总行数只有 1000 行，而不是千百万行）。
   - **优化查询方式（优先原生函数下推）**：
     - 如果执行跨网络连接的大表复杂 GROUP BY 或聚合统计，直接查询三段路径（如 `db_postgres.public.users`）可能会导致所有明细行网络传输极慢。
     - **极力推荐使用原生函数手动下推**。你可以通过 `postgres_query('连接别名', '原生SQL')` 或 `mysql_query('连接别名', '原生SQL')` 将聚合计算语句直接送入远程数据库，计算出结果后再拉回，大幅消减网络延迟。
       - 例如：`SELECT * FROM postgres_query('db_cdp', 'SELECT recipient, COUNT(*) FROM cdp.message_sending_notification GROUP BY recipient')`
       - 注意：连接别名通常对应路径的第一部分（如 `db_cdp.cdp.xxx` 中的 `db_cdp`）。
   - **全表本地缓存落盘策略**：
     - 如果用户需要对某张大表进行**频繁、多次、复杂的交互式 OLAP 分析**，为避免每次运行聚合都重复请求远程连接，你可以调用 `materialize_remote_table` 工具，将该外部表完整物化为 DuckDB 本地物理表。
     - 此时你可以根据任务需求，自主判断是否向用户提议并执行该全量落盘操作。
     - **大表分区物化与增量更新**：`materialize_remote_table` 会自动按时间列或自增ID列分区拉取大表（带进度反馈与超时保护），无需额外指定。
     - **断点续传**：全量物化中途失败（如超时）后，再次调用会自动跳过已物化部分、从断点继续，无需任何额外参数。部分物化期间该表聚合仍会被拦截（数据不完整），直到续传完成。
     - **按需物化**：分析不必整表落盘时，可传 `partitions` 参数只物化需要的区间，例如 `partitions=[{"start":"2025-06-01","end":"2025-07-01"}]`（配合 `partition_strategy=time`）只拉某月数据。按需物化的表为 `partial` 态，聚合会被拦截，需要全量结果时再续传补齐。
     - **增量更新**：对频繁更新的业务大表，若该表已全量物化过且远程有新增数据，可传 `incremental=true` 只补拉增量。

# 禁止行为
- 绝对禁止在对话回复中直接输出任何原始 SVG 代码、HTML 代码片段或 Canvas 渲染代码。若需要向用户可视化展示数据，必须且仅能通过调用专用工具 `render_chart` 来完成，严禁在 Markdown 正文中贴出任何 `<svg>` 或 `<html>` 相关的标签。
- 禁止在总结/结论中使用 `![alt](url)` 等 Markdown 图片语法来引用或嵌入图表——图表不以图片形态存在，无法渲染，只会显示成裸文本 `![...]`。需要在结论中再展示某张已生成的图表时，使用 `render_chart` 返回的 `{{chart:...}}` 标记。
- **绝对禁止编造、挪用、凭印象复述任何数字——这属于「数据纪律（红线）」范畴，违反即视为严重事故，优先级高于本节其余各项。**
- 不要在没有数据支撑时反复猜测
- 不要写"等等"、"不对"、"让我重新想"这类自我纠正的文字
- 不要推翻自己的结论后又得出相同结论
- 不要在一段话中混杂猜测和结论
- 如果数据不足以回答问题，直接说明需要什么数据
- 不要只给出 SQL 文本让用户自己跑——需要加工时就主动用工具创建对象

# 数据库命名规范（创建表/视图时必须遵循）
- `s_`：源文件映射的原始只读视图（如 `s_sales`），可能包含头部备注等脏数据。**只读，不要创建 s_ 开头的对象。**
- `tmp_`：中间过渡物理表（如 `tmp_sales_joined`）。
- `tmp_v_`：中间过渡虚拟视图（如 `tmp_v_order_filtered`）。
- `t_`：最终清洗加工后的可用物理表（如 `t_sales`）。
- `v_`：最终清洗加工后的可用虚拟视图（如 `v_sales`）。

# 思考语言
你的思考过程（reasoning）也必须用中文进行。不要用英文思考，即使问题用英文提出。思考内容应保持与中文回复一致的语言风格。

# 安全约束
- `execute_query` 工具仅用于只读查询（SELECT），禁止通过它执行任何写操作（DELETE, DROP, UPDATE, INSERT, ALTER 等）。
- 所有创建/删除表/视图的操作，只能通过专用工具：`create_table`、`create_view`、`drop_object`。
- 删除操作不可恢复，仅当用户明确要求时才调用 `drop_object`。"#;

// The tool-definition token cost is estimated at runtime in `agent.rs` by
// serializing rig's *actual* `ToolDefinition`s (name + full description + JSON
// Schema parameters) — not a hardcoded approximation — so the "系统工具" slice
// reflects what the model really receives.

/// A single CJK code point (BMP + ext-A + compat + CJK punctuation).
fn is_cjk(ch: char) -> bool {
    matches!(ch,
        '\u{4E00}'..='\u{9FFF}'   // CJK Unified Ideographs
        | '\u{3400}'..='\u{4DBF}' // CJK Extension A
        | '\u{F900}'..='\u{FAFF}' // CJK Compatibility Ideographs
        | '\u{3000}'..='\u{303F}' // CJK Symbols and Punctuation
        | '\u{FF00}'..='\u{FFEF}' // Halfwidth/Fullwidth Forms
    )
}

/// Rough-but-reasonable token estimate from text length.
///
/// Counts **Unicode scalar values** (not bytes — the old `len()/3.5` counted
/// UTF-8 bytes, which triple-counted every Chinese character) and weights by
/// script:
/// - ASCII ≈ 0.25 tok/char (≈ 4 chars/token; matches OpenAI's rule of thumb
///   for English / JSON / code).
/// - CJK ≈ 1.3 tok/char (Chinese is typically 1–2 tokens per character;
///   1.3 is the empirical average for common prose).
/// - Other scripts ≈ 0.5 tok/char.
///
/// This is a heuristic; the per-model calibration factor `k` (see
/// [`fit_calibration`]) corrects its residual bias using real API samples.
pub fn estimate_tokens(s: &str) -> u64 {
    let mut tokens: f64 = 0.0;
    for ch in s.chars() {
        if ch.is_ascii() {
            tokens += 0.25;
        } else if is_cjk(ch) {
            tokens += 1.3;
        } else {
            tokens += 0.5;
        }
    }
    // `f64::ceil` of 0.0 is 0.0; guard NaN/overflow just in case.
    if tokens.is_finite() {
        tokens.ceil().max(0.0) as u64
    } else {
        0
    }
}

/// Uncalibrated (`k = 1`) token estimate of the fixed system prompt.
#[allow(dead_code)]
pub fn raw_preamble_tokens() -> u64 {
    estimate_tokens(PREAMBLE)
}

/// Provider-collapsed, honest usage shape.
///
/// `prompt_tokens` is the **true total prompt** the model was billed for this
/// call (cache-read + cache-creation + fresh). `cache_read_tokens /
/// prompt_tokens` is therefore a cache-hit rate that is always ≤ 100 %,
/// regardless of provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NormalizedUsage {
    /// True total prompt tokens this call (cache read + creation + fresh).
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    /// Tokens served from the provider cache (cheap).
    pub cache_read_tokens: u64,
    /// Tokens written to the provider cache this call.
    pub cache_creation_tokens: u64,
    /// Full-price input tokens (neither cached nor newly-cached).
    pub fresh_input_tokens: u64,
}

/// Collapse provider-specific usage fields into one honest shape.
///
/// - `openai` / `responses` (and any OpenAI-compatible): `input_tokens`
///   **includes** cached tokens; there is no cache-creation concept, so
///   `prompt = input`, `fresh = input - cached`.
/// - `anthropic`: `input_tokens` **excludes** cache; the true prompt is
///   `input + cache_creation + cached`, and `fresh = input`.
///
/// `api_format` is matched case-insensitively; anything not exactly
/// `"anthropic"` is treated as the OpenAI-compatible shape.
pub fn normalize(
    input_tokens: u64,
    output_tokens: u64,
    cached_input_tokens: u64,
    cache_creation_input_tokens: u64,
    api_format: &str,
) -> NormalizedUsage {
    let is_anthropic = api_format.eq_ignore_ascii_case("anthropic");
    if is_anthropic {
        let prompt = input_tokens
            .saturating_add(cache_creation_input_tokens)
            .saturating_add(cached_input_tokens);
        NormalizedUsage {
            prompt_tokens: prompt,
            completion_tokens: output_tokens,
            cache_read_tokens: cached_input_tokens,
            cache_creation_tokens: cache_creation_input_tokens,
            fresh_input_tokens: input_tokens,
        }
    } else {
        // OpenAI-compatible: input_tokens already includes cached tokens.
        NormalizedUsage {
            prompt_tokens: input_tokens,
            completion_tokens: output_tokens,
            cache_read_tokens: cached_input_tokens,
            // OpenAI has no cache-creation concept; rig leaves this 0.
            cache_creation_tokens: cache_creation_input_tokens,
            fresh_input_tokens: input_tokens.saturating_sub(cached_input_tokens),
        }
    }
}

/// The per-model calibration factor `k` is refit by EMA in the **frontend**
/// (`mergeUsage` → `fitK`), because `k` must persist across runs and the
/// backend is stateless between runs. The backend therefore emits a raw
/// `kSample = real_prompt / est_raw` (on the first call of each run, the only
/// call whose prompt we can locally estimate) and lets the frontend smooth it.
/// `fitK` in `src/lib/metrics.ts` mirrors the EMA weight (0.6 / 0.4) used here.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_openai_input_includes_cached() {
        // OpenAI: prompt_tokens=100 (includes 80 cached). hit rate = 80%.
        let n = normalize(100, 10, 80, 0, "openai");
        assert_eq!(n.prompt_tokens, 100);
        assert_eq!(n.cache_read_tokens, 80);
        assert_eq!(n.cache_creation_tokens, 0);
        assert_eq!(n.fresh_input_tokens, 20);
        assert_eq!(n.completion_tokens, 10);
        assert!(n.cache_read_tokens <= n.prompt_tokens);
    }

    #[test]
    fn normalize_anthropic_input_excludes_cache() {
        // Anthropic: input=20 (fresh), creation=50, cached=80 → prompt=150.
        // Old code computed cached/input = 80/20 = 400 % (the >100 % bug).
        let n = normalize(20, 10, 80, 50, "anthropic");
        assert_eq!(n.prompt_tokens, 150);
        assert_eq!(n.cache_read_tokens, 80);
        assert_eq!(n.cache_creation_tokens, 50);
        assert_eq!(n.fresh_input_tokens, 20);
        // Hit rate is now well-formed.
        assert!(n.cache_read_tokens <= n.prompt_tokens);
        assert_eq!(n.cache_read_tokens * 100 / n.prompt_tokens, 53);
    }

    #[test]
    fn normalize_case_insensitive_format() {
        assert_eq!(
            normalize(20, 0, 80, 50, "Anthropic"),
            normalize(20, 0, 80, 50, "anthropic")
        );
        // Unknown format → OpenAI-compatible shape.
        let n = normalize(100, 0, 80, 0, "something-else");
        assert_eq!(n.prompt_tokens, 100);
    }

    #[test]
    fn estimate_tokens_is_char_not_byte_aware() {
        // Pure ASCII: 8 chars → ~2 tokens (0.25/char).
        assert_eq!(estimate_tokens("abcd1234"), 2);
        // 4 Chinese chars → 4*1.3 = 5.2 → ceil 6 tokens. Byte-length/3.5
        // would have counted 12 bytes / 3.5 ≈ 4 — wrong direction for CN.
        assert!(estimate_tokens("你好世界你好") >= 5);
        // Empty → 0, never panics.
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn raw_preamble_is_nonzero() {
        assert!(raw_preamble_tokens() > 100, "preamble should be sizable");
    }
}
