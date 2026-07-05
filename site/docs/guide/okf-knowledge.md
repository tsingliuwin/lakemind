# Open Knowledge Format (OKF) 知识库

在 AI 数据分析的落地中，最核心的痛点往往不是模型能力，而是**上下文在共享与流转中的丢失（Context Disconnection）**。当您把一份 Parquet 文件或数据库连接共享给同事或 AI 时，接收方很难凭空了解：
- 离散表之间的主外键关联（JOIN）关系；
- 专有字段的真实商业语义（如 `status=3` 代表什么）；
- 核心商业指标（如“活跃用户”）的具体计算口径；
- 历史成功的数据清洗排障经验与特殊日期解析配方。

传统做法是将这些信息记录在公司的 Wiki 网站、代码注释或中心化元数据治理系统中，这导致了严重的**上下文破碎与管理沉重**。

为了打破这一僵局，LakeMind 深度采用了由谷歌开源的统一知识表示标准——**Open Knowledge Format (OKF)**。

---

## 什么是 Open Knowledge Format (OKF)？

**Open Knowledge Format (OKF)** 是由谷歌开源的、用于为 AI 智能体（Agent）和人类提供结构化元数据、上下文及治理知识的统一标准。它正式化了 **LLM-wiki** 模式，其核心目的是**通过便携式的上下文打包，彻底改善数据在组织内的协同共享与消费体验**：

1. **“上下文随数据同行”的便携性 (Portable Knowledge Bundles)**：OKF 将元数据和业务逻辑视为第一等资产（First-Class Assets）。在物理上，它就是您工作区目录下的 `.okf/` 文件夹。当您将数据工作区（包含 Parquet 目录或 SQLite 库）打包分享或提交到 Git 仓库时，该知识包会随数据一同流转。
2. **零冷启动分析 (Zero-Cold-Start RAG)**：接收方的 AI Agent 在接入数据后，会秒级扫描并加载工作区的 `.okf/` 规范文件，瞬间“继承”所有关于该数据集的商业常识、关联关系与清洗配方，免去了重新探测数据或反复询问人类专家的开销。
3. **人人可读，机器易懂 (Human & Machine-Readable)**：知识包完全由纯文本的 **Markdown** 文件与 **YAML Front Matter** 组成。人类可以在任意文本编辑器中轻松读写与维护（Git 友好），AI Agent 亦能无缝且结构化地解析其元数据和语义关系。
4. **图状语义关系链 (Semantically Linked Graphs)**：概念文件之间可以通过标准的 Markdown 相对路径链接（如 `[customers](/tables/customers.md)`），将相互独立的数据表、视图与指标织成一张有向无环图（DAG），指引 Agent 沿着关系链进行多表关联（JOIN）和深度推理。

---

## 💡 为什么称为“便携式”知识库？(Why is it "Portable"?）

“便携式（Portable）”是 OKF 最为独特且最具革命性的属性。要理解这一点，我们可以对比传统的企业级元数据治理方案：

传统的数据字典和业务口径通常被存储在**中心化的、极其沉重的云端平台**（如 Collibra、Google Cloud Dataplex 或庞大的中心化关系型元数据库）。这导致了知识在“传输和流转”时的巨大断层：
- **无法离线传输**：一旦你想把一个包含几个 Parquet 文件的项目压缩发给同事，数据字典根本无法随行。
- **环境强力绑定**：获取上下文信息连接需要云环境、安装特定 SDK，或拥有昂贵的平台账户权限。

而 LakeMind 所实现的 OKF 便携式知识库，彻底解决了这一瓶颈。其“便携式”核心体现在以下三个维度：

### 1. 物理位置上的“就地打包随行” (Physical Portability)
OKF 抛弃了任何中心化的元数据库或云端锁定。知识库在物理上就是您项目工作区下的一个隐藏目录 `.okf/`：
- 当您通过 U 盘、网盘或邮件发送整个数据工作区文件夹给同事时，**.okf 文件夹作为数据文件夹的一部分自然同行**。
- 同事在 LakeMind 中打开此工作区的瞬间，其本地 Agent 便能立即消费这些上下文，实现了**数据与业务认知的物理一体化**。

### 2. 消费终端上的“免基础设施依赖” (Infrastructure Independence)
- **人类视角**：因为是 Markdown，您不需要安装 LakeMind 也能查看它。您可以使用 VS Code、Sublime Text，或者在 GitHub 网页上直接阅读和编辑，**没有任何软件锁定**。
- **AI 视角**：标准的 YAML 标头是通用 LLM 和各类智能体框架的“通用语”。无需运行复杂的取数 SDK 或大模型微调，Agent 只需简单读取纯文本即可完美理解字段与指标，**没有任何运行时绑定**。

### 3. 版本与协作上的“Git 友好” (Version-Control Portability)
- 知识随着数据结构的演进而变化（如增加了新字段、修改了指标公式）。
- 由于是纯文本文件，您可以直接将其纳入 Git 进行版本管理。
- 团队成员之间可以通过标准的 Pull Request (PR) 审查指标公式的修改，查看详细的 Diff 历史，甚至一键回滚到以前的版本。**这让商业知识库获得了和软件源代码同等的轻量级协作与工程便携性**。

---

## OKF 在 LakeMind 中的目录结构

在 LakeMind 工作区中，OKF 知识库被组织在 `.okf/` 目录下，包含以下核心语义域：

```
.okf/
├── index.md                 # 工作区全局数据湖概述
├── tables/
│   ├── index.md             # 数据表与视图索引
│   ├── s_taxi_2026.md       # 出租车订单表的 Schema 与关联定义
│   └── s_zones.md           # 区域映射表
├── metrics/
│   ├── index.md             # 商业指标索引
│   └── active_users.md      # “活跃用户”的口径计算公式
└── pipelines/
    ├── index.md             # 数据管道导入经验索引
    └── gbk_csv_loader.md    # 针对 GBK 编码 CSV 导入错误的排障配方
```

---

## OKF 概念文档解析

每一个 OKF 概念文件均由 **YAML Front Matter** 和 **Markdown Body** 组成。

### 示例：表定义文档 (`.okf/tables/s_taxi_2026.md`)

```yaml
---
type: Table
title: s_taxi_2026
description: 纽约出租车 2026 年 6 月的订单明细表，一行代表一次载客行程。
resource: local://lake.duckdb/s_taxi_2026
tags: [taxi, travel, revenue]
timestamp: 2026-07-05T17:00:00Z
---

# Schema

| 字段名 | 数据类型 | 字段描述 | 关联关系 |
| :--- | :--- | :--- | :--- |
| `vendor_id` | VARCHAR | 运营商 ID | |
| `pulocationid` | INTEGER | 上车区域编码 | 关联 [s_zones](/tables/s_zones.md) 的 `locationid` |
| `fare_amount` | DOUBLE | 乘客行程的车费金额 | |

# Joins

当需要探查行程发生的具体地点名称时，使用 `pulocationid` 关联 [s_zones](/tables/s_zones.md) 表。
```

> [!IMPORTANT]
> **唯一必填字段**：OKF 规范保持最小侵入性，仅强制要求 Front Matter 中包含 `type` 字段（例如 `Table`、`Metric` 或 `Pipeline`），以让 Agent 识别文件类型。其余描述性字段和 Body 部分的结构均可高度自由定义。

---

## Agent 如何与 OKF 协同工作

LakeMind 的 Agent 通过内置的 `rig` 工具链，实现了对 OKF 知识库的**双向读写与自动整理**：

### 1. 知识发现与消费 (Consuming)
- **语义搜索**：当您用自然语言提问时，Agent 会首先调用 `search_okf_recipes` 在知识库中模糊检索与问题相关的表（`tables/`）和口径（`metrics/`）。
- **SQL 辅助生成**：Agent 读取解析好的 Schema 和 Joins 链接，准确获取列名和外键关系，自动编写出正确的 DuckDB 多表关联 SQL，彻底杜绝大模型的字段幻觉。

### 2. 知识沉淀与规整 (Producing)
- **经验记录**：在对话中成功处理完一个异常数据文件后，Agent 可以通过 `write_okf_block` 将解决方案（例如特定编码的 CSV Loader 参数）写入到 `pipelines/` 中。
- **知识库整理 (Tidy)**：Agent 可以启动 `tidy_okf_knowledge` 整理任务。它会用 ```okf-file ``` 格式输出重组后的文件。在写入本地磁盘时，LakeMind 后端会使用“备份 $\rightarrow$ 写入 $\rightarrow$ 校验 $\rightarrow$ 出错回滚”的原子事务机制，确保本地的 OKF 知识库永远处于健康、不破损的状态。
