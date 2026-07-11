# 快速入门 (Getting Started)

跟随本指南，在几分钟内安装/启动 LakeMind，配置您的 AI 大模型，并开始用自然语言探索本地数据。

---

## 1. 直接下载安装（推荐）

如果您不需要修改 LakeMind 源码或参与开发，可以直接下载我们预先编译打包好的正式版本，开箱即用：

- **下载地址**：[GitHub Releases 官方下载通道](https://github.com/tsingliuwin/lakemind/releases)

---

## 2. 从源码编译与启动（开发者）

作为一个开源项目，如果您是开发者，想要定制软件或贡献代码，可以按照以下步骤在本地进行编译与启动。

首先，确保您的开发环境已安装 [Node.js](https://nodejs.org/) 以及 Rust 编译链（Tauri 开发基础）。

```bash
# 1. 克隆代码库并安装依赖
git clone https://github.com/tsingliuwin/lakemind.git
cd lakemind
npm install --include=dev

# 2. 启动 Tauri 开发服务器 (首次编译需要 5-15 分钟下载并编译 DuckDB 源码)
npm run tauri dev
```

> [!TIP]
> 首次编译会因为编译 Rust 版嵌入式 DuckDB 引擎而比较缓慢，此后再次启动将直接使用编译缓存，秒级启动。

---

## 3. 配置 AI 大模型 (LLM)

LakeMind 依靠大语言模型的推理与自纠错能力来驱动 Agent 运行。为了获得最佳分析性能、极速响应与极致的低成本，我们**推荐配置云端 API 密钥**。

> [!IMPORTANT]
> **关于数据隐私**：无论使用何种大模型 API，LakeMind 与大模型交互时**仅传输表结构（Schema）和 OKF 语义定义，原始数据明细行绝对不上网**。所有的查询、清洗与多步加工都在本地 DuckDB 中执行，完美保障隐私！

### 配置大模型 API 密钥（推荐，零部署门槛）
这是最快、最省心也是分析效果最好的使用方式。我们特别推荐使用 **DeepSeek-v4-flash** 等快且省的轻量模型，或者 **OpenAI gpt-4o** 等旗舰模型。
1. 获取您的 AI 平台 API 密钥（例如 DeepSeek 平台或 OpenAI 平台）。
2. 打开 LakeMind，点击右上角齿轮进入 **设置 (Settings)**。
3. 在模型提供商中选择您的提供商（如 `DeepSeek` 或 `OpenAI`），填入 API 密钥（API Key）。
4. 如果使用第三方转发，可自定义 API Endpoint 地址。模型选择推荐速度飞快、Token 极其低廉的 `deepseek-v4-flash` 或 `gpt-4o`。

---

## 4. 创建您的第一个工作区

1. 启动应用后，点击首页的 **+ 新建工作区 (New Workspace)**。
2. 选择本地的一个空文件夹（例如 `~/Downloads/my_first_lake`）。
3. LakeMind 会自动在该文件夹下创建工作区配置文件，并初始化专属的本地数据库 `lake.duckdb`。此时您的本地 Lakehouse 已经创建完毕！

---

## 5. 导入数据并与 Agent 对话

### 第一步：导入本地数据或连接数据库
LakeMind 不仅支持本地文件分析，还能直接对接各类关系型数据库：
- **导入本地文件**（支持 `CSV`、`Parquet`、`Excel`、`Delta`）：
  - 直接在系统中把文件或整个文件夹**拖拽**到 LakeMind 主窗口的空白区域。
  - 在左侧导航栏的 **Files** 面板中浏览本地磁盘，右键点击文件选择 **Import to Data Lake**。
- **连接外部数据库**（已支持 `SQLite` 等，未来将不断集成更多数据库类型）：
  - 在左侧导航栏点击 **Connections**（连接配置），点击 **Add Database Connection**。
  - 填入本地或远程数据库的连接路径/参数，Agent 会自动扫描其 Schema 并织入 OKF 本地知识库。

### 第二步：开始自然语言对话
1. 按下快捷键 `⌘ + Shift + N`（Windows 下为 `Ctrl + Shift + N`）新建一个 Chat 对话任务。
2. 在下方对话框中输入大白话：
   > “分析下这个表里有哪些列，找出包含缺失值最多的列，并以饼图展现缺失值比例。”
3. 回车发送。接下来，Agent 会自动：
   - 🔍 调用 `list_tables` 和 `describe_table` 探查元数据。
   - ⚙️ 调用 `execute_query` 计算统计信息。
   - 📊 调用 `render_chart` 将饼图渲染呈现在对话框里。
