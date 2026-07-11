# LakeMind 分析准则库

本准则是 LakeMind 从真实案例中提炼的数据分析方法论体系。它随软件分发，所有工作区共享。动手分析前，先查这里有没有相关准则，比现想更可靠。

浏览方式：先看本大纲，定位到相关准则后，用 `load_tenets` 按 concept ID 加载全文精读；不确定关键词时用 `search_tenets` 检索。

## 核心准则

核心准则分为**总则**（根本原则）、**数据纪律红线**、**分则**（按阶段）和**准则变更准则**（元规则）：

### 总则
* [总则：根本原则](core/general-principles.md) — 实事求是、数据为证、归属正确、最小假设、先理解后分析。

### 数据纪律红线
* [数据纪律](core/data-discipline.md) — 不可触碰的五条红线：禁止编造、禁止张冠李戴、三问自检、禁止截断、先怀疑自己。

### 分则（按数据分析阶段）
* [数据画像](core/data-profiling.md) — 如何了解你的数据：时间范围、数据质量、字段含义、数据量级。
* [数据清洗](core/data-cleaning.md) — 从原始数据到高质量数据表：脏数据识别、清洗策略、可追溯原则。
* [数据分析](core/data-analysis.md) — 从数据到洞察：假设驱动、多维验证、统计陷阱规避。
* [数据呈现](core/data-presentation.md) — 如何展示分析结果：图表选择、数字精度、结论表达。

### 准则变更准则
* [准则的准则](core/meta-governance.md) — 准则本身的修改也需要规范：生命周期、来源要求、层级关系。

## 行业准则与案例

各行业在数据上的典型陷阱与正反案例。行业准则也有总则（行业通用）和分则（子行业特有）的层级结构。

* [教育](industry/education/index.md) — 课程销售、体验课转化等场景的典型陷阱。
  * [K12](industry/education/k12.md) — 小学/初中/高中课外辅导。
  * [考研](industry/education/postgrad.md) — 考研培训。（待积累）
* [旅游](industry/tourism.md) — （待积累）
* [房地产](industry/realestate.md) — （待积累）

## 分析主题准则

按分析主题组织的通用方法论，跨行业适用。

* [转化分析](topic/conversion.md) — 漏斗归因、口径一致性、重复转化去重等。
* [用户增长](topic/growth.md) — （待积累）
