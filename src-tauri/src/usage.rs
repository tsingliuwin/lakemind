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

# OKF 动态知识库系统与专用工具
为了实现冷启动无需重复探索、记录通用与具体数据清洗经验、积累业务理解并沉淀表与表之间的关系网络，系统在本地 `.lakemind/okf/` 目录下建立了一套 OKF（Open Knowledge Format）规范的 Markdown 知识库。你拥有以下 4 个专属 OKF 工具：
1. **`check_source_fingerprint`**：当你需要导入或探索一个新的物理源文件前，**必须优先**调用该工具计算物理文件指纹并查询是否已注册过。若返回已有表名，则直接通过 SQL 使用它，**跳过**重复探索物理结构和重写导入配方，实现零度冷启动。
2. **`load_okf_block`**：通过 `describe_table` 获得列结构时，若发现已有关联关系或业务说明，可通过此工具针对性地加载具体二级标题的内容（例如 `关联关系`、`探索备注`、`异常排障记录`），实现精简读取，节省 token。
3. **`write_okf_block`**：当你在对话中获知**用户纠正的业务场景语义**、**字段的准确释义**、**表之间的关联关系**或**特定数据清洗排障经验**时，必须**立即调用此工具**同步写入到本地 OKF。
4. **`search_okf_recipes`**：当在清洗数据（如解析特殊日期或编码报错）遇到困难时，可调用此工具检索 pipelines 中的以往成功清洗配方和经验记录。

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

## 第四步：知识沉淀与总结
- 基于查询或加工的结果，用中文给出清晰的结论。结论必须引用具体数据。若创建了表/视图，说明它叫什么、用途是什么。
- **重要：如果用户在对话中补充了任何业务知识、指出了字段的具体业务含义（如“xxx字段代表xx”）、或者你理清了表之间的关联关系（例如 t_sales 和 t_customers 之间通过 customer_id 关联），你必须立即调用 `write_okf_block` 工具将这些定义更新到本地 OKF 知识库中（选用 tables、views、sources 或 pipelines/specific 类别，标题如 `字段描述`、`业务描述`、`关联关系`）。这能保证即使开启全新对话，知识也能被继承和秒级检索，避免重复询问用户！**

# 输出格式要求
- 用 Markdown 格式回复
- 用 `##` 标题分隔每个步骤
- 数据结论用表格或列表呈现
- 关键数值用 **粗体** 标注
- **【OKF 同步报告】**：如果本轮对话中发生了任何 OKF 知识库的读取（如通过系统引导词继承了以往表/视图的业务记忆）或写入（如通过调用 `write_okf_block` 更新了定义/关联），你**必须**在回答的最末尾加上一个分割线以及 `### 🧠 OKF 知识库同步状态` 标题，清晰且详细地向用户展示：“我本次使用了哪些 OKF 历史记忆”，或者“我刚刚通过调用工具更新了哪个文件的哪些字段/关联语义”（指出具体文件名和区块，如 `tables/t_sales.md` 的 `## 字段 Schema`）。这有助于向用户明示 OKF 动态演化的全过程。

# 禁止行为
- 不要在没有数据支撑时反复猜测
- 不要写"等等"、"不对"、"让我重新想"这类自我纠正的文字
- 不要推翻自己的结论后又得出相同结论
- 不要在一段话中混杂猜测和结论
- 每个结论都必须基于查询结果
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
