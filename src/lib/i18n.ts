import { createSignal } from "solid-js";

export type Language = "zh" | "en";

export const [currentLanguage, setCurrentLanguage] = createSignal<Language>("zh");

const translations: Record<Language, Record<string, string>> = {
  zh: {
    // LeftNav
    newQuery: "新建查询",
    search: "搜索",
    skills: "技能",
    workspace: "工作区",
    dragHint: "拖入文件夹或文件以开始",
    interfaceLanguage: "界面语言",
    interfaceTheme: "界面主题",
    interfaceZoom: "界面缩放",
    settings: "设置",
    remainingQuota: "剩余额度",
    feedback: "问题反馈",
    community: "用户社群",
    disconnect: "断开连接",
    langZh: "简体中文",
    langEn: "English",
    themeGeekDark: "极客暗黑",
    themeClassicDark: "经典深色",
    themeLight: "极致浅色",
    quotaLocal: "本地纯算力",
    quotaUnlimited: "额度无限制",
    showInspector: "显示检查器",
    hideInspector: "隐藏检查器",
    showConsole: "显示控制台",
    hideConsole: "隐藏控制台",

    // SqlEditor
    sqlEditorTag: "📊 DuckDB SQL",
    rowCountLimit: "行数上限",
    copySql: "复制 SQL",
    run: "运行",
    running: "执行中…",

    // RightInspector
    inspectorEmptyHint: "选中一个源表以查看其 Schema。",
    rowsUnit: "行",
    colsUnit: "列",
    fieldsLabel: "字段",
    previewRowsBtn: "▶ 预览 50 行",

    // BottomConsole
    consoleTitle: "控制台",
    consoleEmptyHint: "还没有执行过查询。按运行（Ctrl/Cmd+Enter）。",
    clearLog: "清空日志",
    expandConsole: "展开控制台",
    expandFurther: "进一步展开",
    foldConsole: "折叠",
    latest: "最近",

    // TitleBar dropdown & modal
    newQueryTask: "新建任务",
    openInExplorer: "在资源管理器中打开",
    openExplorerSelectFirst: "请先在左侧选择一个数据源表！",
    modelSettingsCenter: "模型设置中心",
    aboutApp: "关于 ZCode/LakeMind",
    checkUpdates: "检查更新",
    closeWindow: "关闭窗口",
    minimize: "最小化",
    maximize: "最大化",
    close: "关闭",
    currentSource: "当前源",
    aboutTitle: "⚓ 关于 LakeMind Client",
    aboutCore: "LakeMind M1 — 纯算力客户端",
    aboutDesc: "基于 Tauri 2.0 与 DuckDB 的本地优先数据分析终端。",
    aboutVersion: "版本",
    aboutKernel: "内核",
    aboutEnv: "运行环境",
    aboutArch: "引擎架构",
    okBtn: "确定",
    settingsM1Placeholder: "设置面板将在 M4 上线。M1 目前是纯算力客户端。",
    latestVersionMsg: "已是最新版本 M1 (v0.1.0)",
  },
  en: {
    // LeftNav
    newQuery: "New Query",
    search: "Search",
    skills: "Skills",
    workspace: "Workspace",
    dragHint: "Drag folders/files here to start",
    interfaceLanguage: "Language",
    interfaceTheme: "Theme",
    interfaceZoom: "Zoom",
    settings: "Settings",
    remainingQuota: "Quota",
    feedback: "Feedback",
    community: "Community",
    disconnect: "Disconnect",
    langZh: "Simplified Chinese",
    langEn: "English",
    themeGeekDark: "Geek Dark",
    themeClassicDark: "Classic Dark",
    themeLight: "Premium Light",
    quotaLocal: "Local Compute Only",
    quotaUnlimited: "Unlimited Quota",
    showInspector: "Show Inspector",
    hideInspector: "Hide Inspector",
    showConsole: "Show Console",
    hideConsole: "Hide Console",

    // SqlEditor
    sqlEditorTag: "📊 DuckDB SQL",
    rowCountLimit: "Row Limit",
    copySql: "Copy SQL",
    run: "Run",
    running: "Running...",

    // RightInspector
    inspectorEmptyHint: "Select a source table to view its schema.",
    rowsUnit: "rows",
    colsUnit: "cols",
    fieldsLabel: "Fields",
    previewRowsBtn: "▶ Preview 50 Rows",

    // BottomConsole
    consoleTitle: "Console",
    consoleEmptyHint: "No query has been executed yet. Press Run (Ctrl/Cmd+Enter).",
    clearLog: "Clear Logs",
    expandConsole: "Expand Console",
    expandFurther: "Expand Further",
    foldConsole: "Fold",
    latest: "latest",

    // TitleBar dropdown & modal
    newQueryTask: "New Query",
    openInExplorer: "Open in File Explorer",
    openExplorerSelectFirst: "Please select a source table first!",
    modelSettingsCenter: "Model Settings",
    aboutApp: "About ZCode/LakeMind",
    checkUpdates: "Check for Updates",
    closeWindow: "Close Window",
    minimize: "Minimize",
    maximize: "Maximize",
    close: "Close",
    currentSource: "Source",
    aboutTitle: "⚓ About LakeMind Client",
    aboutCore: "LakeMind M1 — Pure-Compute Client",
    aboutDesc: "A local-first data analytics terminal powered by Tauri 2.0 and DuckDB.",
    aboutVersion: "Version",
    aboutKernel: "Kernel",
    aboutEnv: "Environment",
    aboutArch: "Architecture",
    okBtn: "OK",
    settingsM1Placeholder: "Settings panel will release in M4. M1 is currently a pure-compute local client.",
    latestVersionMsg: "Already at the latest version M1 (v0.1.0)",
  }
};

export function t(key: string): string {
  const lang = currentLanguage();
  return translations[lang][key] || translations["zh"][key] || key;
}
