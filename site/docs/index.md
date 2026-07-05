---
# https://vitepress.dev/reference/default-theme-home-page
layout: home

hero:
  name: "LakeMind"
  text: "本地优先的 AI 智能数据探索工作台"
  tagline: "打通本地文件与关系数据库的零 ETL 混合联邦执行。基于极致低延迟的“快速试错-纠偏环”，融合谷歌开源 OKF 标准，让 Agent 真正深入数据清洗、多步加工与业务记忆积累。"
  actions:
    - theme: brand
      text: 快速入门 (LLM配置)
      link: /guide/getting-started
    - theme: alt
      text: 了解架构
      link: /guide/architecture
---

<!-- 1. CSS 桌面端 UI 模拟器 -->
<!-- 1. 演示视频展示 -->
<div class="video-section" style="margin-top: 24px; text-align: center;">
  <h2 class="section-head-title">演示视频展示 (Demo Videos)</h2>
  <p class="section-head-desc" style="max-width: 600px; margin: 8px auto 24px auto;">
    观看真实录制的运行视频，直观感受 LakeMind 如何作为您的本地智能助手，助您秒级完成数据探查、SQL 生成与图表渲染。
  </p>
  <div class="video-container" style="max-width: 800px; margin: 0 auto; border-radius: 12px; overflow: hidden; border: 1px solid rgba(255, 255, 255, 0.08); box-shadow: 0 25px 50px -12px rgba(0, 0, 0, 0.5), 0 0 40px rgba(6, 182, 212, 0.04); background: #0b0f19;">
    <div style="position: relative; padding-bottom: 56.25%; height: 0;">
      <iframe 
        src="https://www.youtube.com/embed/videoseries?list=PLZ14dE31D5Bc" 
        title="LakeMind Demo Videos"
        style="position: absolute; top: 0; left: 0; width: 100%; height: 100%; border: 0;" 
        allow="accelerometer; autoplay; clipboard-write; encrypted-media; gyroscope; picture-in-picture; web-share" 
        allowfullscreen>
      </iframe>
    </div>
  </div>
</div>

<!-- Creator's Note -->
<div class="creator-note-section" style="max-width: 800px; margin: 56px auto; padding: 28px 36px; border-radius: 16px; background: rgba(6, 182, 212, 0.02); border: 1px dashed rgba(6, 182, 212, 0.15); text-align: center; box-shadow: inset 0 0 20px rgba(6, 182, 212, 0.01);">
  <p style="font-size: 1.12rem; font-style: italic; line-height: 1.65; color: var(--vp-c-text-1); margin: 0 0 12px 0;">
    “很多问数产品纠结于如何让大模型尽可能写对 SQL，为此设计了各种沉重的模式。但我认为，<b>花 2 分钟冥思苦想写出 1 个所谓正确的语句，不如花 1 分钟根据真实数据和报错快速尝试 10 次</b>。Agent 对数据分析的本质改变，就是通过极致的本地低延迟反馈，提升人工探索数据的效率。”
  </p>
  <span style="font-size: 0.85rem; font-weight: 600; color: #06b6d4; text-transform: uppercase; letter-spacing: 0.08em;">— LakeMind 开发者自白</span>
</div>

<!-- 2. Agent 核心能力展示 -->
<div class="comparison-section">
<h2 class="section-head-title">为什么选择 LakeMind Agent</h2>
<p class="section-head-desc">本地离线引擎 + 智能 Agent 协同，为您解锁前所未有的数据分析体验</p>

<div class="features-grid">
<div class="feature-card">
<span class="feature-icon">🤖</span>
<h3 class="feature-title">自然语言直接对话</h3>
<p class="feature-desc">无需手写 SQL 语句。只需以大白话提出您的问题，本地 Agent 将自动执行表结构探查、多表关联优化，自主为您编写并安全运行 SQL 查询。</p>
</div>

<div class="feature-card">
<span class="feature-icon">🔒</span>
<h3 class="feature-title">本地优先与零 ETL 联邦探索</h3>
<p class="feature-desc">数据 100% 留存本地。与大模型交互仅发送表结构 Schema，原始数据行绝不上网。无需漫长管道与同步，支持在本地对异构多源数据直接跨源联合（JOIN）分析。</p>
</div>

<div class="feature-card">
<span class="feature-icon">📊</span>
<h3 class="feature-title">智能可视化抉择</h3>
<p class="feature-desc">Agent 具备数据敏感的呈现决策脑：对账与单值等精确数字场景以纯表格呈现；对包含趋势、对比、占比的统计行则自动调用 ECharts 智能图表渲染。</p>
</div>

<div class="feature-card">
<span class="feature-icon">🧠</span>
<h3 class="feature-title">谷歌开源 OKF 统一标准</h3>
<p class="feature-desc">采用谷歌开源的 Open Knowledge Format (OKF) 统一知识表示标准，将 Schema、主外键 JOIN 关系网与指标定义打包为便携式上下文，彻底消除数据共享中的语义断层。</p>
</div>

<div class="feature-card">
<span class="feature-icon">⚡</span>
<h3 class="feature-title">高容错多策略加载</h3>
<p class="feature-desc">针对混乱的业务导出文件，Loader 提供 5 种 Excel 表头质量评分算法，及 CSV 编码与分隔符自动嗅探，确保各种现实脏数据能顺利解析。</p>
</div>

<div class="feature-card">
<span class="feature-icon">💼</span>
<h3 class="feature-title">隔离的多工作区设计</h3>
<p class="feature-desc">每个工作区拥有独立的本地 DuckDB 实例与元数据索引。超大文件支持 In-place 注册免去磁盘占用，小规模数据支持便携式复制归档以供同事开箱即用。</p>
</div>
</div>
</div>

<!-- 3. 产品对比 -->
<div class="comparison-section">
<h2 class="section-head-title">产品对比</h2>
<p class="section-head-desc">我们将本地隐私保护、大模型智能交互与强劲计算性能融合为一</p>

<div class="comp-table-wrapper">
<table class="comp-table">
<thead>
<tr>
<th>维度</th>
<th style="color: #10b981;">LakeMind</th>
<th>普通问数工具 (如云端 Text-to-SQL)</th>
<th>传统 SQL 客户端 (如 DBeaver)</th>
</tr>
</thead>
<tbody>
<tr>
<td>数据安全与隐私</td>
<td class="highlight">100% 本地 (仅发送 Schema，原始行绝不上网)</td>
<td>原始数据明细需上传云端 (高泄露合规风险)</td>
<td>100% 本地 (但完全无智能交互)</td>
</tr>
<tr>
<td>多源异构联邦探索</td>
<td class="highlight">本地混合联邦执行 (桌面文件与数据库直接 JOIN)</td>
<td>不支持本地文件，或需将其全部上传至云端数仓</td>
<td>无法跨源查询 (必须通过繁重 ETL 提取物理拼表)</td>
</tr>
<tr>
<td>反馈与自我纠错</td>
<td class="highlight">高频毫秒级本地“试错-修正”环 (智能快速自愈)</td>
<td>单次生成模式 (反应慢，运行报错需要人工排查)</td>
<td>报错完全依赖分析师人工排障与重写 SQL</td>
</tr>
<tr>
<td>单次查询成本</td>
<td class="highlight">普惠低成本 (完美释放 deepseek-v4-flash 等省快模型)</td>
<td>高成本 (强绑定 GPT-4/Claude 等重型旗舰大模型)</td>
<td>零 Token 成本 (但耗费极高的人工手写时间与开销)</td>
</tr>
<tr>
<td>分析结果沉淀</td>
<td class="highlight">Agent 自主物化落盘 (自动建 t_ / v_ 资产表)</td>
<td>“问完即走” (只显示静态表，无法将结果就地落盘)</td>
<td>必须由分析师手动编写建表 DDL 并执行维护</td>
</tr>
<tr>
<td>知识积累与流转</td>
<td class="highlight">谷歌开源 OKF 统一标准 (Markdown 随行，Git 友好)</td>
<td>保存在云端私有格式或无保存 (语义流转即断层)</td>
<td>仅本地保存 SQL 历史脚本记录 (无语义背景)</td>
</tr>
</tbody>
</table>
</div>
</div>

<!-- 4. 技术栈栈底展示 -->
<div class="stack-section">
<h2 class="section-head-title">高性能现代技术栈</h2>
<p class="section-head-desc">基于 Rust 生态构建的极致底层，让分析飞速流畅</p>

<div class="stack-grid">
<div class="stack-card">
<div class="tech-icon">🦀</div>
<div class="tech-name">Tauri 2.0 & Rust</div>
<div class="tech-desc">高性能桌面安全沙盒，包体积小，内存开销极低。</div>
</div>
<div class="stack-card">
<div class="tech-icon">🦆</div>
<div class="tech-name">Embedded DuckDB</div>
<div class="tech-desc">嵌入式 OLAP 引擎，专为分析 Parquet/Delta 而生。</div>
</div>
<div class="stack-card">
<div class="tech-icon">🤖</div>
<div class="tech-name">Rig Agent SDK</div>
<div class="tech-desc">Rust 级智能体开发套件，驱动 14 种高效交互工具。</div>
</div>
<div class="stack-card">
<div class="tech-icon">⚡</div>
<div class="tech-name">零拷贝二进制 IPC</div>
<div class="tech-desc">基于 Tauri 2.0 的二进制数据管道，数据传递免 JSON 解析，百万行秒级无感交互。</div>
</div>
</div>
</div>
