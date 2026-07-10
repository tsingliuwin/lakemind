import { createSignal, Show, onMount, For } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import Select from "./Select";
import { t, currentLanguage, setCurrentLanguage } from "../lib/i18n";
import { currentTheme, setCurrentTheme, currentZoom, setCurrentZoom, logoSrc } from "../lib/theme";
import {
  codeFontSize, setCodeFontSizeP,
  codeLineNumbers, setCodeLineNumbersP,
  codeWrap, setCodeWrapP,
} from "../lib/codeConfig";
import type { DbConnection } from "../lib/types";
import { selectFile } from "../lib/duckdb";
import { logError } from "../lib/logger";

const isMac = typeof navigator !== "undefined" && navigator.userAgent.includes("Mac");

export type SettingsTab =
  | "general"
  | "databases"
  | "codePreview"
  | "modelSettings"
  | "skills"
  | "mcp"
  | "plugins"
  | "commands"
  | "indexDb"
  | "stats"
  | "guide";

/** 暂时隐藏的设置 tab：这些功能尚未实现，导航项先不展示。
 *  未来逐步补充内容后，从此集合移除对应项即可恢复显示。 */
const HIDDEN_TABS = new Set<SettingsTab>([
  "skills",
  "mcp",
  "plugins",
  "indexDb",
  "stats",
  "guide",
]);

/* 主题选项的小图标（月亮 = 暗色、太阳 = 浅色），线条风格，替代 emoji。 */
function MoonIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 13px; height: 13px;">
      <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"></path>
    </svg>
  );
}
function SunIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 13px; height: 13px;">
      <circle cx="12" cy="12" r="5"></circle>
      <line x1="12" y1="1" x2="12" y2="3"></line>
      <line x1="12" y1="21" x2="12" y2="23"></line>
      <line x1="4.22" y1="4.22" x2="5.64" y2="5.64"></line>
      <line x1="18.36" y1="18.36" x2="19.78" y2="19.78"></line>
      <line x1="1" y1="12" x2="3" y2="12"></line>
      <line x1="21" y1="12" x2="23" y2="12"></line>
      <line x1="4.22" y1="19.78" x2="5.64" y2="18.36"></line>
      <line x1="18.36" y1="5.64" x2="19.78" y2="4.22"></line>
    </svg>
  );
}

export interface ModelItem {
  id: string;
  contextWindow: number;
  maxTokens?: number;
}

export interface ModelProvider {
  id: string;
  name: string;
  endpoint: string;
  apiKey: string;
  apiFormat: "openai" | "anthropic" | "responses";
  models: ModelItem[];
  enabled: boolean;
  isPredefined?: boolean;
}

export interface AppSettings {
  theme?: string;
  language?: string;
  zoom?: string;
  providers?: ModelProvider[];
  [key: string]: any;
}

// Per-model connection-test status icon — 4 states, one shape each so it's
// legible without color. Used by both the existing-provider and new-provider
// model lists. On the error state: hover shows the detail tooltip, click copies
// the error message to the clipboard (with a brief "已复制" feedback in place of
// the tooltip). There is no separate copy button — the icon itself is the copy
// affordance, which keeps the test-button row clean.
//
// IMPORTANT (SolidJS): the function body runs ONCE; do NOT destructure props
// into a local const (e.g. `const s = props.status`) — that freezes the value
// and the icon never updates. Branch via <Show> so each `props.status` read
// happens inside a reactive boundary.
function ModelStatusIcon(props: {
  status: "idle" | "testing" | "success" | "error" | undefined;
  errorTip?: string;
}) {
  const [copied, setCopied] = createSignal(false);
  return (
    <Show
      when={props.status === "testing"}
      fallback={
        <Show
          when={props.status === "success"}
          fallback={
            <Show
              when={props.status === "error"}
              fallback={<span class="mt-status idle" title="未测试" />}
            >
              <span
                class="mt-error-wrap"
                tabIndex={0}
                title={t("clickToCopyError")}
                onClick={async (e) => {
                  e.stopPropagation();
                  try {
                    await navigator.clipboard.writeText(props.errorTip || "");
                    setCopied(true);
                    setTimeout(() => setCopied(false), 1500);
                  } catch {}
                }}
              >
                <span class="mt-status error">✕</span>
                <Show when={props.errorTip}>
                  <span class="mt-error-tip">
                    {copied() ? t("copied") : props.errorTip}
                  </span>
                </Show>
              </span>
            </Show>
          }
        >
          <span class="mt-status success" title="可用">✓</span>
        </Show>
      }
    >
      <div class="mt-spin" />
    </Show>
  );
}

export default function SettingsPage(props: {
  workspacePath?: string;
  onClose: () => void;
  onOpenSettings?: () => void;
  titleBar?: any;
  /** Initial sub-section to land on when settings opens (defaults to "general").
   * Lets callers deep-link to a specific tab, e.g. "databases". */
  initialTab?: SettingsTab;
}) {
  const [activeTab, setActiveTab] = createSignal<SettingsTab>(props.initialTab ?? "general");

  // Collapsible settings sidebar: when true the left rail shrinks to a narrow
  // strip exposing only the expand toggle, giving the right detail panel full
  // width. Mirrors the main shell's left-sidebar collapse affordance.
  const [sidebarCollapsed, setSidebarCollapsed] = createSignal(false);

  // Selection signals
  const [selectedProvider, setSelectedProvider] = createSignal<string>("");
  const [isAddingProvider, setIsAddingProvider] = createSignal(false);

  const [settings, setSettings] = createSignal<AppSettings>({
    providers: []
  });

  const [showApiKey, setShowApiKey] = createSignal(false);

  // Rename fields inline
  const [editingProviderId, setEditingProviderId] = createSignal<string | null>(null);
  const [tempName, setTempName] = createSignal("");

  // Database sampling state
  const [sampleEnabled, setSampleEnabled] = createSignal(true);
  const [sampleLimit, setSampleLimit] = createSignal(10000);



  // New provider temp states
  const [newProviderName, setNewProviderName] = createSignal("");
  const [newProviderEndpoint, setNewProviderEndpoint] = createSignal("");
  const [newProviderApiKey, setNewProviderApiKey] = createSignal("");
  const [newProviderFormat, setNewProviderFormat] = createSignal<"openai" | "anthropic" | "responses">("openai");
  const [newProviderModels, setNewProviderModels] = createSignal<ModelItem[]>([]);

  // Dialog popups for adding/editing models
  const [isModelModalOpen, setIsModelModalOpen] = createSignal(false);
  const [isAddingToTempProvider, setIsAddingToTempProvider] = createSignal(false);
  const [modalMode, setModalMode] = createSignal<"add" | "edit">("add");
  const [editingModelId, setEditingModelId] = createSignal<string>("");
  const [modelFormId, setModelFormId] = createSignal("");
  const [modelFormWindow, setModelFormWindow] = createSignal(200000);
  const [modelFormMaxTokens, setModelFormMaxTokens] = createSignal(4096);

  // Database connections state
  const [connections, setConnections] = createSignal<DbConnection[]>([]);
  const [editingConn, setEditingConn] = createSignal<DbConnection | null>(null);
  const [testStatus, setTestStatus] = createSignal<{ status: "idle" | "testing" | "success" | "error"; msg?: string }>({ status: "idle" });
  const [copiedTestError, setCopiedTestError] = createSignal(false);
  // Per-model connection-test state. Nested map: providerKey → modelId → result.
  // `providerKey` is the provider id for saved providers, or NEW_PROVIDER_TEST_KEY
  // for the add-new-provider form. In-memory only (not persisted): a provider must
  // have all its models validated in THIS session before it can be switched from
  // disabled → enabled. Already-enabled providers are unaffected on restart.
  type ModelTestEntry = { status: "idle" | "testing" | "success" | "error"; msg?: string };
  const [modelTests, setModelTests] = createSignal<Record<string, Record<string, ModelTestEntry>>>({});
  const [batchProgress, setBatchProgress] = createSignal<{ providerKey: string; current: number; total: number; modelId: string } | null>(null);
  // Sentinel key for the "add new provider" form (which has no provider id yet).
  const NEW_PROVIDER_TEST_KEY = "__new__";
  const [linkedConns, setLinkedConns] = createSignal<Record<string, boolean>>({});

  const [formName, setFormName] = createSignal("");
  const [formType, setFormType] = createSignal<"postgres" | "mysql" | "sqlite">("postgres");
  const [formHost, setFormHost] = createSignal("");
  const [formPort, setFormPort] = createSignal(5432);
  const [formDatabase, setFormDatabase] = createSignal("");
  const [formUsername, setFormUsername] = createSignal("");
  const [formPassword, setFormPassword] = createSignal("");
  const [formSslMode, setFormSslMode] = createSignal("disable");
  const [showPassword, setShowPassword] = createSignal(false);
  const [formUri, setFormUri] = createSignal("");
  const [uriStatus, setUriStatus] = createSignal<{ status: "idle" | "success" | "error"; errorMsg?: string }>({ status: "idle" });

  const loadConnections = async () => {
    try {
      const list = await invoke<DbConnection[]>("get_db_connections");
      setConnections(list);
    } catch (err) {
      logError("ui", "Failed to load db connections", err);
    }
  };

  const loadWorkspaceLinks = async () => {
    if (!props.workspacePath) return;
    try {
      const linked = await invoke<DbConnection[]>("list_workspace_connections", { wsPath: props.workspacePath });
      const map: Record<string, boolean> = {};
      for (const c of linked) {
        map[c.id] = true;
      }
      setLinkedConns(map);
    } catch (err) {
      logError("ui", "Failed to load workspace connections", err);
    }
  };

  const handleToggleLink = async (connId: string) => {
    if (!props.workspacePath) return;
    const isLinked = linkedConns()[connId];
    try {
      if (isLinked) {
        await invoke("unlink_connection_from_workspace", { wsPath: props.workspacePath, connId });
      } else {
        await invoke("link_connection_to_workspace", { wsPath: props.workspacePath, connId });
      }
      loadWorkspaceLinks();
    } catch (err) {
      alert("操作失败: " + err);
    }
  };

  const handleSaveConnection = async () => {
    const name = formName().trim();
    if (!name) {
      alert("请输入连接名称");
      return;
    }

    const isSqlite = formType() === "sqlite";
    // SQLite only needs a file path; network/auth fields are unused.
    if (isSqlite) {
      const filePath = formDatabase().trim();
      if (!filePath) {
        alert("请选择数据库文件");
        return;
      }
      const connData: DbConnection = {
        id: editingConn()?.id || "conn_" + Date.now(),
        name,
        dbType: "sqlite",
        host: "",
        port: 0,
        databaseName: filePath,
        username: "",
        password: "",
        sslMode: "disable",
        createdAt: editingConn()?.createdAt || Date.now(),
      };
      try {
        await invoke("upsert_db_connection", { config: connData });
        setEditingConn(null);
        loadConnections();
      } catch (err) {
        alert("保存失败: " + err);
      }
      return;
    }

    const host = formHost().trim();
    if (!host) {
      alert("请输入主机地址");
      return;
    }
    const databaseName = formDatabase().trim();
    if (!databaseName) {
      alert("请输入数据库名");
      return;
    }
    const username = formUsername().trim();
    if (!username) {
      alert("请输入用户名");
      return;
    }

    const connData: DbConnection = {
      id: editingConn()?.id || "conn_" + Date.now(),
      name,
      dbType: formType(),
      host,
      port: formPort(),
      databaseName,
      username,
      password: formPassword(),
      sslMode: formSslMode(),
      createdAt: editingConn()?.createdAt || Date.now(),
    };

    try {
      await invoke("upsert_db_connection", { config: connData });
      setEditingConn(null);
      loadConnections();
    } catch (err) {
      alert("保存失败: " + err);
    }
  };

  const handleTestConnection = async () => {
    const name = formName().trim();
    const isSqlite = formType() === "sqlite";

    if (isSqlite) {
      const filePath = formDatabase().trim();
      if (!name || !filePath) {
        alert("请填写连接名称并选择数据库文件以测试");
        return;
      }
      const connData: DbConnection = {
        id: editingConn()?.id || "test_temp",
        name,
        dbType: "sqlite",
        host: "",
        port: 0,
        databaseName: filePath,
        username: "",
        password: "",
        sslMode: "disable",
        createdAt: Date.now(),
      };
      setTestStatus({ status: "testing" });
      try {
        await invoke("test_db_connection", { config: connData });
        setTestStatus({ status: "success", msg: "连接成功！" });
      } catch (err) {
        setTestStatus({ status: "error", msg: "测试失败: " + err });
      }
      return;
    }

    const host = formHost().trim();
    const databaseName = formDatabase().trim();
    const username = formUsername().trim();

    if (!name || !host || !databaseName || !username) {
      alert("请填写基础连接信息以测试");
      return;
    }

    const connData: DbConnection = {
      id: editingConn()?.id || "test_temp",
      name,
      dbType: formType(),
      host,
      port: formPort(),
      databaseName,
      username,
      password: formPassword(),
      sslMode: formSslMode(),
      createdAt: Date.now(),
    };

    setTestStatus({ status: "testing" });
    try {
      await invoke("test_db_connection", { config: connData });
      setTestStatus({ status: "success", msg: "连接成功！" });
    } catch (err) {
      setTestStatus({ status: "error", msg: "测试失败: " + err });
    }
  };

  // Set one model's test entry inside the nested map (immutable update so
  // SolidJS reactivity fires on the relevant row).
  const setModelEntry = (providerKey: string, modelId: string, entry: ModelTestEntry) => {
    setModelTests((prev) => ({
      ...prev,
      [providerKey]: { ...(prev[providerKey] || {}), [modelId]: entry },
    }));
  };

  // Discard all test results for a provider. Called when the user edits any
  // config field (endpoint / apiKey / apiFormat / model list) — a stale result
  // from the old config would be misleading (e.g. a red ✕ after switching the
  // API format that actually fixes it). Also clears the batch-in-progress flag.
  const clearModelTests = (providerKey: string) => {
    setModelTests((prev) => {
      if (!(providerKey in prev)) return prev;
      const next = { ...prev };
      delete next[providerKey];
      return next;
    });
    if (batchProgress()?.providerKey === providerKey) {
      setBatchProgress(null);
    }
  };

  // Whether every model under a provider is currently validated as available
  // (all in "success" state). Used by the enable gate and the dot indicator.
  // Returns false if there are no models or any model isn't a success.
  const providerValidated = (providerKey: string, models: ModelItem[]): boolean => {
    if (!models.length) return false;
    const entries = modelTests()[providerKey] || {};
    return models.every((m) => entries[m.id]?.status === "success");
  };

  // Whether a batch test is currently running for a given provider key.
  const batchTesting = (providerKey: string): boolean =>
    batchProgress()?.providerKey === providerKey;

  // Batch-test EVERY model of a provider, sequentially (avoids hammering the API
  // and tripping rate limits). Updates per-model state as it goes and reports
  // progress. `providerKey` is the provider id (or NEW_PROVIDER_TEST_KEY for the
  // add form). `endpoint/apiKey/apiFormat` come from the form so unsaved edits
  // are reflected. Returns true iff ALL models passed.
  const runBatchModelTest = async (
    providerKey: string,
    endpoint: string,
    apiKey: string,
    apiFormat: string,
    models: ModelItem[],
  ): Promise<boolean> => {
    const ep = (endpoint || "").trim();
    const key = (apiKey || "").trim();
    if (!ep || !key || !models.length) {
      // Mark all models as error with the empty-config hint.
      for (const m of models) {
        setModelEntry(providerKey, m.id, { status: "error", msg: t("modelTestEmpty") });
      }
      return false;
    }
    let allOk = true;
    for (let i = 0; i < models.length; i++) {
      const m = models[i];
      setBatchProgress({ providerKey, current: i + 1, total: models.length, modelId: m.id });
      setModelEntry(providerKey, m.id, { status: "testing" });
      try {
        await invoke("test_llm_connection", {
          endpoint: ep,
          apiKey: key,
          apiFormat,
          modelId: m.id,
        });
        setModelEntry(providerKey, m.id, { status: "success" });
      } catch (err) {
        setModelEntry(providerKey, m.id, { status: "error", msg: String(err) });
        allOk = false;
      }
    }
    setBatchProgress(null);
    return allOk;
  };

  // Existing-provider detail form: batch-test all of a saved provider's models
  // against its live (possibly unsaved) form values.
  const handleTestModelConnection = (prov: ModelProvider) =>
    runBatchModelTest(prov.id, prov.endpoint, prov.apiKey, prov.apiFormat, prov.models || []);

  // Add-new-provider form: batch-test the temp models before saving.
  const handleTestNewProviderConnection = () =>
    runBatchModelTest(
      NEW_PROVIDER_TEST_KEY,
      newProviderEndpoint(),
      newProviderApiKey(),
      newProviderFormat(),
      newProviderModels(),
    );

  const handleDeleteConnection = async (id: string) => {
    if (!confirm("确定要删除该数据库连接吗？这将从所有关联的项目中解绑！")) {
      return;
    }
    try {
      await invoke("delete_db_connection", { id });
      loadConnections();
    } catch (err) {
      alert("删除失败: " + err);
    }
  };

  const startEditConnection = (c: DbConnection) => {
    setEditingConn(c);
    setFormName(c.name);
    setFormType(c.dbType);
    setFormHost(c.host);
    setFormPort(c.port);
    setFormDatabase(c.databaseName);
    setFormUsername(c.username);
    setFormPassword(c.password || "");
    setFormSslMode(c.sslMode || "disable");
    setShowPassword(false);
    setFormUri("");
    setUriStatus({ status: "idle" });
    setTestStatus({ status: "idle" });
  };

  const startAddConnection = () => {
    setEditingConn({
      id: "",
      name: "local_postgres",
      dbType: "postgres",
      host: "localhost",
      port: 5432,
      databaseName: "postgres",
      username: "postgres",
    });
    setFormName("local_postgres");
    setFormType("postgres");
    setFormHost("localhost");
    setFormPort(5432);
    setFormDatabase("postgres");
    setFormUsername("postgres");
    setFormPassword("");
    setFormSslMode("disable");
    setShowPassword(false);
    setFormUri("");
    setUriStatus({ status: "idle" });
    setTestStatus({ status: "idle" });
  };

  const parseConnectionString = (str: string): { success: true; data: any } | { success: false; error: string } => {
    try {
      const clean = str.trim();
      if (!clean.includes("://")) {
        return { success: false, error: "缺失协议头，请以 postgresql:// 或 mysql:// 开头" };
      }
      
      let url: URL;
      try {
        url = new URL(clean);
      } catch (e) {
        return { success: false, error: "格式不正确，请检查语法（例如特殊字符未进行 URL 编码）" };
      }
      
      const protocol = url.protocol.toLowerCase();
      if (protocol !== "postgresql:" && protocol !== "postgres:" && protocol !== "mysql:") {
        return { success: false, error: "仅支持 postgresql:// 或 mysql:// 协议" };
      }
      
      const dbType = protocol === "mysql:" ? "mysql" : "postgres";
      const host = url.hostname;
      if (!host) {
        return { success: false, error: "连接串中缺失主机名 (Host)" };
      }
      
      const port = url.port ? parseInt(url.port) : (dbType === "mysql" ? 3306 : 5432);
      const username = decodeURIComponent(url.username || "");
      const password = decodeURIComponent(url.password || "");
      
      let databaseName = url.pathname;
      if (databaseName.startsWith("/")) {
        databaseName = databaseName.substring(1);
      }
      databaseName = decodeURIComponent(databaseName);
      
      let sslMode = "disable";
      if (url.searchParams.has("sslmode")) {
        sslMode = url.searchParams.get("sslmode") || "disable";
      }
      
      return {
        success: true,
        data: {
          dbType,
          host,
          port,
          databaseName,
          username,
          password,
          sslMode
        }
      };
    } catch (err: any) {
      return { success: false, error: err.message || "未知解析错误" };
    }
  };

  const handleUriInput = (e: { currentTarget: HTMLInputElement }) => {
    const val = e.currentTarget.value.trim();
    if (!val) {
      setUriStatus({ status: "idle" });
      return;
    }
    const result = parseConnectionString(val);
    if (result.success) {
      const parsed = result.data;
      setFormType(parsed.dbType);
      setFormHost(parsed.host);
      setFormPort(parsed.port);
      setFormDatabase(parsed.databaseName);
      setFormUsername(parsed.username);
      setFormPassword(parsed.password);
      setFormSslMode(parsed.sslMode);
      
      // Auto name if default or empty
      if (formName() === "" || formName() === "local_postgres" || formName() === "local_mysql") {
        const cleanHost = parsed.host.split('.')[0];
        setFormName(`${cleanHost}_${parsed.databaseName}`);
      }
      
      setUriStatus({ status: "success" });
    } else {
      setUriStatus({ status: "error", errorMsg: result.error });
    }
  };

    onMount(async () => {
    try {
      const json = await invoke<string>("load_settings_json");
      if (json && json !== "{}") {
        const loaded = JSON.parse(json);
        setSettings(loaded);
        
        if (loaded.providers && loaded.providers.length > 0) {
          setSelectedProvider(loaded.providers[0].id);
        }
      }
    } catch (err) {
      logError("ui", "Failed to load settings", err);
    }

    // Load database sampling configurations
    try {
      const enabledVal = await invoke<string | null>("get_app_config", { key: "explore.materialized_sample_enabled" });
      if (enabledVal !== null) {
        setSampleEnabled(enabledVal === "true");
      }
      const limitVal = await invoke<string | null>("get_app_config", { key: "explore.materialized_sample_limit" });
      if (limitVal !== null) {
        const parsed = parseInt(limitVal, 10);
        if (!isNaN(parsed)) {
          setSampleLimit(parsed);
        }
      }
    } catch (err) {
      logError("ui", "Failed to load app config settings", err);
    }

    loadConnections();
    loadWorkspaceLinks();
  });

  // Save settings helper
  const updateSetting = (key: keyof AppSettings, value: any) => {
    const updated = { ...settings(), [key]: value };
    setSettings(updated);
    invoke("save_settings_json", { json: JSON.stringify(updated, null, 2) }).catch(err => {
      logError("ui", "Failed to save settings", err);
    });
  };

  const handleToggleSampleEnabled = async (enabled: boolean) => {
    setSampleEnabled(enabled);
    try {
      await invoke("set_app_config", { key: "explore.materialized_sample_enabled", value: enabled ? "true" : "false" });
    } catch (err) {
      logError("ui", "Failed to save explore.materialized_sample_enabled", err);
    }
  };

  const handleUpdateSampleLimit = async (limit: number) => {
    setSampleLimit(limit);
    try {
      await invoke("set_app_config", { key: "explore.materialized_sample_limit", value: limit.toString() });
    } catch (err) {
      logError("ui", "Failed to save explore.materialized_sample_limit", err);
    }
  };

  const updateProviderProperty = (providerId: string, property: keyof ModelProvider, value: any) => {
    // Editing any field that affects connectivity invalidates prior test
    // results — reset them so stale ✓/✕ icons don't mislead.
    if (property === "endpoint" || property === "apiKey" || property === "apiFormat" || property === "models") {
      clearModelTests(providerId);
    }
    const updatedProviders = (settings().providers || []).map(p => {
      if (p.id === providerId) {
        return { ...p, [property]: value };
      }
      return p;
    });
    updateSetting("providers", updatedProviders);
  };

  const handleSaveProviderName = () => {
    const val = tempName().trim();
    if (val && editingProviderId()) {
      updateProviderProperty(editingProviderId()!, "name", val);
    }
    setEditingProviderId(null);
  };

  const handleDeleteProvider = (id: string) => {
    const updated = (settings().providers || []).filter(p => p.id !== id);
    updateSetting("providers", updated);
    if (updated.length > 0) {
      setSelectedProvider(updated[0].id);
    } else {
      setSelectedProvider("");
    }
  };

  // Add a new provider, then batch-test every model. Provider starts disabled;
  // it is auto-enabled only if ALL models pass validation, otherwise stays
  // disabled (the user can fix the config and re-test, then enable manually).
  const handleCreateNewProvider = async () => {
    const name = newProviderName().trim();
    const endpoint = newProviderEndpoint().trim();
    const apiKey = newProviderApiKey().trim();
    const format = newProviderFormat();
    const models = newProviderModels();

    if (!name) {
      alert("请输入服务商名称");
      return;
    }
    if (!endpoint) {
      alert("请输入 Base URL");
      return;
    }
    if (!models.length) {
      alert(t("modelTestEmpty"));
      return;
    }

    const newId = "custom_" + Date.now();
    // Always create disabled first — enable depends on the batch test result.
    const newProvider: ModelProvider = {
      id: newId,
      name,
      endpoint,
      apiKey,
      apiFormat: format,
      models,
      enabled: false,
    };

    const updated = [...(settings().providers || []), newProvider];
    updateSetting("providers", updated);
    setSelectedProvider(newId);
    setIsAddingProvider(false);

    // Reset temp states
    setNewProviderName("");
    setNewProviderEndpoint("");
    setNewProviderApiKey("");
    setNewProviderFormat("openai");
    setNewProviderModels([]);

    // Batch-test against the just-saved provider (keyed by its new id). If all
    // models pass, flip enabled on automatically.
    const allOk = await runBatchModelTest(newId, endpoint, apiKey, format, models);
    if (allOk) {
      updateProviderProperty(newId, "enabled", true);
    }
  };

  // Model actions handlers
  const handleOpenAddModel = () => {
    setIsAddingToTempProvider(false);
    setModalMode("add");
    setModelFormId("");
    setModelFormWindow(200000);
    setIsModelModalOpen(true);
  };

  const handleOpenAddTempModel = () => {
    setIsAddingToTempProvider(true);
    setModalMode("add");
    setModelFormId("");
    setModelFormWindow(200000);
    setModelFormMaxTokens(4096);
    setIsModelModalOpen(true);
  };

  const handleOpenEditModel = (model: ModelItem) => {
    setIsAddingToTempProvider(false);
    setModalMode("edit");
    setEditingModelId(model.id);
    setModelFormId(model.id);
    setModelFormWindow(model.contextWindow);
    setModelFormMaxTokens(model.maxTokens || 4096);
    setIsModelModalOpen(true);
  };

  const handleOpenEditTempModel = (model: ModelItem) => {
    setIsAddingToTempProvider(true);
    setModalMode("edit");
    setEditingModelId(model.id);
    setModelFormId(model.id);
    setModelFormWindow(model.contextWindow);
    setModelFormMaxTokens(model.maxTokens || 4096);
    setIsModelModalOpen(true);
  };

  const handleDeleteModel = (modelId: string) => {
    const currentProv = (settings().providers || []).find(p => p.id === selectedProvider());
    if (!currentProv) return;
    const updatedModels = currentProv.models.filter(m => m.id !== modelId);
    updateProviderProperty(selectedProvider(), "models", updatedModels);
  };

  const handleSaveModel = () => {
    const mId = modelFormId().trim();
    if (!mId) return;

    if (isAddingToTempProvider()) {
      if (modalMode() === "add") {
        if (newProviderModels().some(m => m.id === mId)) {
          alert("模型已存在");
          return;
        }
        setNewProviderModels([...newProviderModels(), { id: mId, contextWindow: modelFormWindow(), maxTokens: modelFormMaxTokens() }]);
      } else {
        setNewProviderModels(newProviderModels().map(m => {
          if (m.id === editingModelId()) {
            return { id: mId, contextWindow: modelFormWindow(), maxTokens: modelFormMaxTokens() };
          }
          return m;
        }));
      }
      clearModelTests(NEW_PROVIDER_TEST_KEY);
      setIsModelModalOpen(false);
      return;
    }

    const currentProv = (settings().providers || []).find(p => p.id === selectedProvider());
    if (!currentProv) return;

    let updatedModels: ModelItem[] = [];
    if (modalMode() === "add") {
      if (currentProv.models.some(m => m.id === mId)) {
        alert("模型已存在");
        return;
      }
      updatedModels = [...currentProv.models, { id: mId, contextWindow: modelFormWindow(), maxTokens: modelFormMaxTokens() }];
    } else {
      updatedModels = currentProv.models.map(m => {
        if (m.id === editingModelId()) {
          return { id: mId, contextWindow: modelFormWindow(), maxTokens: modelFormMaxTokens() };
        }
        return m;
      });
    }

    updateProviderProperty(selectedProvider(), "models", updatedModels);
    setIsModelModalOpen(false);
  };



  return (
    <div class="settings-layout-wrapper">
      <style>{`
        .db-icon-btn {
          display: inline-flex;
          align-items: center;
          justify-content: center;
          width: 32px;
          height: 32px;
          padding: 0 !important;
          border: 1px solid transparent !important;
          border-radius: 6px;
          background: transparent;
          color: var(--text-dim);
          cursor: pointer;
          transition: all 0.15s ease;
        }
        .db-icon-btn:hover {
          background: var(--bg-hover) !important;
          color: var(--text-primary);
          border-color: transparent !important;
        }
        .db-icon-btn.active {
          background: transparent !important;
          color: var(--brand);
        }
        .db-icon-btn.active:hover {
          background: var(--bg-hover) !important;
          color: var(--brand);
          border-color: transparent !important;
        }
        .db-icon-btn.btn-delete:hover {
          background: rgba(255, 80, 80, 0.12);
          color: var(--text-danger);
          border-color: transparent !important;
        }
        .btn-prim-no-highlight {
          border: 1px solid transparent !important;
          transition: all 0.15s ease;
        }
        .btn-prim-no-highlight:hover {
          border-color: transparent !important;
          opacity: 0.9;
        }
        .sp-input:hover:not(:focus) {
          border-color: var(--border-strong) !important;
        }
        .sp-input:focus {
          border-color: var(--border-strong) !important;
          background: var(--bg-active) !important;
          outline: none !important;
          box-shadow: none !important;
        }
        .btn-sec-no-highlight {
          border: 1px solid var(--border-strong) !important;
          transition: all 0.15s ease;
        }
        .btn-sec-no-highlight:hover {
          border-color: var(--border-strong) !important;
          background: var(--bg-hover) !important;
        }
        .loader-spinner {
          width: 13px;
          height: 13px;
          border: 2px solid var(--border-strong);
          border-top: 2px solid var(--brand);
          border-radius: 50%;
          animation: loader-spin 0.8s linear infinite;
        }
        @keyframes loader-spin {
          0% { transform: rotate(0deg); }
          100% { transform: rotate(360deg); }
        }
      `}</style>
      {/* Settings Sidebar */}
      <aside class="settings-sidebar" classList={{ collapsed: sidebarCollapsed() }}>
        <div class="ss-logo-area" classList={{ "mac-nav": isMac }}>
          <Show when={!isMac}>
            <div class="ss-logo-box">
              <img src={logoSrc()} alt="LakeMind" style="width: 18px; height: 18px; object-fit: contain;" />
            </div>
          </Show>
          <button
            class="ss-collapse-btn"
            classList={{ active: !sidebarCollapsed() }}
            title={sidebarCollapsed() ? "展开侧栏" : "折叠侧栏"}
            onClick={() => setSidebarCollapsed(!sidebarCollapsed())}
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect>
              <line x1="9" y1="3" x2="9" y2="21"></line>
            </svg>
          </button>
        </div>

        <button class="ss-back-btn" onClick={() => props.onClose()}>
          <span class="back-icon">←</span>
          <span class="back-label">{t("backToWorkspace")}</span>
        </button>

        <nav class="ss-nav">
          <button 
            class="ss-nav-item" 
            classList={{ active: activeTab() === "general" }}
            onClick={() => setActiveTab("general")}
          >
            <span class="ss-nav-icon">
              <svg class="ss-nav-svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                <circle cx="4" cy="5" r="1.5" />
                <path d="M5.5 5h8M2 5h0.5" />
                <circle cx="12" cy="11" r="1.5" />
                <path d="M2 11h8.5M13.5 11h0.5" />
              </svg>
            </span>
            <span>{t("settingsGeneral")}</span>
          </button>

          <button 
            class="ss-nav-item" 
            classList={{ active: activeTab() === "databases" }}
            onClick={() => setActiveTab("databases")}
          >
            <span class="ss-nav-icon">
              <svg class="ss-nav-svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                <ellipse cx="8" cy="4.5" rx="5" ry="2" />
                <path d="M3 4.5v3.5c0 1.1 2.24 2 5 2s5-.9 5-2v-3.5" />
                <path d="M3 8v3.5c0 1.1 2.24 2 5 2s5-.9 5-2v-3.5" />
              </svg>
            </span>
            <span>{t("settingsDatabases")}</span>
          </button>

          <button 
            class="ss-nav-item" 
            classList={{ active: activeTab() === "codePreview" }}
            onClick={() => setActiveTab("codePreview")}
          >
            <span class="ss-nav-icon">
              <svg class="ss-nav-svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                <path d="M5 11L2 8l3-3M11 5l3 3-3 3M9.5 4l-3 8" />
              </svg>
            </span>
            <span>{t("settingsCodePreview")}</span>
          </button>

          <button 
            class="ss-nav-item" 
            classList={{ active: activeTab() === "modelSettings" }}
            onClick={() => setActiveTab("modelSettings")}
          >
            <span class="ss-nav-icon">
              <svg class="ss-nav-svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                <rect x="2" y="3" width="12" height="4" rx="1" />
                <rect x="2" y="9" width="12" height="4" rx="1" />
                <circle cx="4.5" cy="5" r="0.5" fill="currentColor" />
                <circle cx="4.5" cy="11" r="0.5" fill="currentColor" />
              </svg>
            </span>
            <span>{t("modelSettings")}</span>
          </button>

          <button
            class="ss-nav-item"
            classList={{ active: activeTab() === "skills" }}
            onClick={() => setActiveTab("skills")}
            style={{ display: HIDDEN_TABS.has("skills") ? "none" : undefined }}
          >
            <span class="ss-nav-icon">
              <svg class="ss-nav-svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                <path d="M3 13l7.5-7.5" />
                <path d="M12.5 1.5l.5 1.5.5-1.5zM14 3.5l-1.5.5 1.5.5zM10.5 3l.5-.5-.5-.5z" />
                <circle cx="10.5" cy="5.5" r="0.75" fill="currentColor" />
                <circle cx="7.5" cy="2.5" r="0.75" fill="currentColor" />
              </svg>
            </span>
            <span>{t("settingsSkills")}</span>
          </button>

          <button
            class="ss-nav-item"
            classList={{ active: activeTab() === "mcp" }}
            onClick={() => setActiveTab("mcp")}
            style={{ display: HIDDEN_TABS.has("mcp") ? "none" : undefined }}
          >
            <span class="ss-nav-icon">
              <svg class="ss-nav-svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                <path d="M9 3v4a3 3 0 0 1-6 0V3M4 1v2M8 1v2M6 10v4" />
              </svg>
            </span>
            <span>{t("settingsMcp")}</span>
          </button>

          <button
            class="ss-nav-item"
            classList={{ active: activeTab() === "plugins" }}
            onClick={() => setActiveTab("plugins")}
            style={{ display: HIDDEN_TABS.has("plugins") ? "none" : undefined }}
          >
            <span class="ss-nav-icon">
              <svg class="ss-nav-svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                <path d="M10 2.5c0-.83-.67-1.5-1.5-1.5S7 1.67 7 2.5v1.2H4.5A1.3 1.3 0 0 0 3.2 5v2.5h1.2c.83 0 1.5.67 1.5 1.5S5.23 10.5 4.4 10.5H3.2V13a1.3 1.3 0 0 0 1.3 1.3H7v-1.2c0-.83.67-1.5 1.5-1.5s1.5.67 1.5 1.5v1.2h2.5a1.3 1.3 0 0 0 1.3-1.3V10.5h-1.2c-.83 0-1.5-.67-1.5-1.5s.67-1.5 1.5-1.5h1.2V5A1.3 1.3 0 0 0 12.5 3.7H10V2.5z" />
              </svg>
            </span>
            <span>{t("settingsPlugins")}</span>
          </button>

          <button 
            class="ss-nav-item" 
            classList={{ active: activeTab() === "commands" }}
            onClick={() => setActiveTab("commands")}
          >
            <span class="ss-nav-icon">
              <svg class="ss-nav-svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                <path d="M3 4l4 4-4 4M9 12h4" />
              </svg>
            </span>
            <span>{t("settingsCommands")}</span>
          </button>

          <button
            class="ss-nav-item"
            classList={{ active: activeTab() === "indexDb" }}
            onClick={() => setActiveTab("indexDb")}
            style={{ display: HIDDEN_TABS.has("indexDb") ? "none" : undefined }}
          >
            <span class="ss-nav-icon">
              <svg class="ss-nav-svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                <path d="M8 2s4.5 1.5 4.5 4.5v3.5c0 2.5-2.5 4.5-4.5 5-2-.5-4.5-2.5-4.5-5V6.5C3.5 3.5 8 2 8 2z" />
                <path d="M6 8.5l1.5 1.5 3-3" />
              </svg>
            </span>
            <span>{t("settingsIndexDb")}</span>
          </button>

          <button
            class="ss-nav-item"
            classList={{ active: activeTab() === "stats" }}
            onClick={() => setActiveTab("stats")}
            style={{ display: HIDDEN_TABS.has("stats") ? "none" : undefined }}
          >
            <span class="ss-nav-icon">
              <svg class="ss-nav-svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                <path d="M2.5 13.5h11M4.5 13.5v-3M8 13.5v-6M11.5 13.5V3.5" />
              </svg>
            </span>
            <span>{t("settingsStats")}</span>
          </button>

          <Show when={!HIDDEN_TABS.has("guide")}>
            <div class="ss-guide-container">
              <button
                class="ss-nav-item"
                classList={{ active: activeTab() === "guide" }}
                onClick={() => setActiveTab("guide")}
              >
                <span class="ss-nav-icon">
                  <svg class="ss-nav-svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M12.5 3.5c-1.5-1.5-5 0-6 1a25 25 0 0 0-2.5 3.5L2 9.5l2.5 2L6 14l1.5-2c1.5-.8 2.8-1.8 3.8-3.3 1-1 2.5-4.5 1.2-5.7zM4.5 11.5L2 14M9.5 6.5l.5.5" />
                  </svg>
                </span>
                <span>{t("settingsGuide")}</span>
              </button>
            </div>
          </Show>
        </nav>

        <div class="ss-footer">
          <button
            class="ss-brand"
            onClick={() => props.onClose()}
            title={t("backToWorkspace")}
          >
            <img src={logoSrc()} alt="LakeMind" style="width: 18px; height: 18px; object-fit: contain;" />
            <span class="ss-brand-name">LakeMind</span>
          </button>
          <button
            class="ss-theme-toggle"
            onClick={() => setCurrentTheme(currentTheme() === "light" ? "geek-dark" : "light")}
            title={t("interfaceTheme")}
          >
            <Show when={currentTheme() === "light"} fallback={
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
                <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"></path>
              </svg>
            }>
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
                <circle cx="12" cy="12" r="5"></circle>
                <line x1="12" y1="1" x2="12" y2="3"></line>
                <line x1="12" y1="21" x2="12" y2="23"></line>
                <line x1="4.22" y1="4.22" x2="5.64" y2="5.64"></line>
                <line x1="18.36" y1="18.36" x2="19.78" y2="19.78"></line>
                <line x1="1" y1="12" x2="3" y2="12"></line>
                <line x1="21" y1="12" x2="23" y2="12"></line>
                <line x1="4.22" y1="19.78" x2="5.64" y2="18.36"></line>
                <line x1="18.36" y1="5.64" x2="19.78" y2="4.22"></line>
              </svg>
            </Show>
          </button>
        </div>
      </aside>

      {/* Settings Right Area */}
      <div class="settings-right-container">
        {props.titleBar}
        {/* Settings Main Content Area */}
        <main class="settings-content">
        
        {/* Tab: Databases Settings (数据库) */}
        <Show when={activeTab() === "databases"}>
          <div class="settings-view-header" style="display: flex; justify-content: space-between; align-items: flex-start; max-width: 680px; margin-bottom: 24px;">
            <div>
              <h2>{t("settingsDatabases")}</h2>
              <p class="settings-view-subtitle" style="margin-top: 4px;">{t("settingsDatabasesDesc")}</p>
            </div>
            <Show when={!editingConn()}>
              <button 
                class="ss-btn btn-prim-no-highlight" 
                style="padding: 6px 16px; font-size: 13px; font-weight: 500; border-radius: 8px; box-shadow: 0 2px 8px rgba(0, 0, 0, 0.15);" 
                onClick={startAddConnection}
              >
                {t("newConnectionBtn")}
              </button>
            </Show>
          </div>

          <Show when={!editingConn()} fallback={
            <div class="settings-section-card" style="padding: 24px; display: flex; flex-direction: column; gap: 20px; margin-top: 16px; max-width: 680px; border-radius: 12px; background: var(--bg-surface); border: 1px solid var(--border-strong); box-shadow: 0 8px 32px rgba(0, 0, 0, 0.24);">
              
              {/* Header */}
              <div style="display: flex; justify-content: space-between; align-items: center; border-bottom: 1px solid var(--border-faint); padding-bottom: 14px;">
                <h3 style="margin: 0; font-size: 15px; font-weight: 600; color: var(--text-primary);">
                  {editingConn()?.id ? t("editConnTitle") : t("addConnTitle")}
                </h3>
                <button class="ss-btn ss-btn-secondary btn-sec-no-highlight" style="padding: 4px 12px; font-size: 12.5px; border-radius: 6px;" onClick={() => setEditingConn(null)}>
                  {t("cancelBtn")}
                </button>
              </div>

              {/* Connection URI Import — not applicable to file-based SQLite */}
              <Show when={formType() !== "sqlite"}>
              <div style="display: flex; flex-direction: column; gap: 6px;">
                <label style="font-size: 11px; font-weight: 600; color: var(--text-dim); text-transform: uppercase; letter-spacing: 0.5px;">连接串 (Connection URI)</label>
                <div style="position: relative; display: flex; align-items: center; width: 100%;">
                  <input
                    type="text"
                    class="sp-input"
                    style="padding-left: 30px; width: 100%;"
                    value={formUri()}
                    placeholder="例: postgresql://username:password@host:port/database"
                    onInput={(e) => {
                      setFormUri(e.currentTarget.value);
                      handleUriInput(e);
                    }}
                  />
                  <div 
                    style={`position: absolute; left: 10px; top: 0; bottom: 0; display: flex; align-items: center; justify-content: center; pointer-events: none; z-index: 10; ${
                      uriStatus().status === "success" ? "color: var(--text-success);" :
                      uriStatus().status === "error" ? "color: var(--text-danger);" :
                      "color: var(--text-dim);"
                    }`}
                  >
                    <Show when={uriStatus().status !== "idle"} fallback={
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 13px; height: 13px; opacity: 0.6;">
                        <path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/>
                        <path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/>
                      </svg>
                    }>
                      <Show when={uriStatus().status === "success"} fallback={
                        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" style="width: 13px; height: 13px;">
                          <circle cx="12" cy="12" r="10"/>
                          <line x1="12" y1="8" x2="12" y2="12"/>
                          <line x1="12" y1="16" x2="12.01" y2="16"/>
                        </svg>
                      }>
                        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" style="width: 13px; height: 13px;">
                          <path d="M22 11.08V12a10 10 0 1 1-5.93-9.14"/>
                          <polyline points="22 4 12 14.01 9 11.01"/>
                        </svg>
                      </Show>
                    </Show>
                  </div>
                </div>
                
                {/* Specific Error message under the input */}
                <Show when={uriStatus().status === "error" && uriStatus().errorMsg}>
                  <span style="font-size: 10px; color: var(--text-danger); display: flex; align-items: center; gap: 4px; margin-top: 2px;">
                    {uriStatus().errorMsg}
                  </span>
                </Show>
              </div>
              </Show>

              {/* Database Type Card Selector */}
              <div style="display: flex; flex-direction: column; gap: 8px;">
                <label style="font-size: 11px; font-weight: 600; color: var(--text-dim); text-transform: uppercase; letter-spacing: 0.5px;">{t("dbType")}</label>
                <div style="display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr) minmax(0, 1fr); gap: 14px;">
                  {/* PostgreSQL Card */}
                  <div 
                    onClick={() => {
                      setFormType("postgres");
                      if (formPort() === 3306 || formPort() === 0) setFormPort(5432);
                      if (formName() === "local_mysql" || formName() === "local_sqlite" || formName() === "") setFormName("local_postgres");
                      if (formUsername() === "root" || formUsername() === "") setFormUsername("postgres");
                      if (formDatabase() === "mysql" || formDatabase() === "") setFormDatabase("postgres");
                    }}
                    style={`display: flex; align-items: center; gap: 14px; padding: 12px 18px; border-radius: 10px; border: 2px solid transparent; background: ${formType() === "postgres" ? "rgba(80, 160, 255, 0.06)" : "rgba(255, 255, 255, 0.015)"}; cursor: pointer; transition: all 0.2s ease-in-out; box-shadow: ${formType() === "postgres" ? "0 4px 12px rgba(80, 160, 255, 0.1)" : "none"}`}
                    class="db-type-card"
                  >
                    <div style="display: flex; align-items: center; justify-content: center; width: 34px; height: 34px; background: rgba(80, 160, 255, 0.12); color: var(--brand); border-radius: 8px; flex-shrink: 0;">
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="width: 18px; height: 18px;">
                        <ellipse cx="12" cy="5" rx="9" ry="3"></ellipse>
                        <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5"></path>
                        <path d="M3 12c0 1.66 4 3 9 3s9-1.34 9-3"></path>
                      </svg>
                    </div>
                    <div style="flex: 1; min-width: 0;">
                      <div style="font-weight: 600; font-size: 13.5px; color: var(--text-primary);">PostgreSQL</div>
                      <div style="font-size: 11px; color: var(--text-dim); margin-top: 2px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">{t("dbTypePostgresDesc")}</div>
                    </div>
                    <span 
                      style={`color: var(--brand); font-size: 15px; font-weight: bold; margin-left: auto; transition: opacity 0.15s ease-in-out; ${
                        formType() === "postgres" ? "opacity: 1;" : "opacity: 0; pointer-events: none;"
                      }`}
                    >
                      ✓
                    </span>
                  </div>

                  {/* MySQL Card */}
                  <div 
                    onClick={() => {
                      setFormType("mysql");
                      if (formPort() === 5432 || formPort() === 0) setFormPort(3306);
                      if (formName() === "local_postgres" || formName() === "local_sqlite" || formName() === "") setFormName("local_mysql");
                      if (formUsername() === "postgres" || formUsername() === "") setFormUsername("root");
                      if (formDatabase() === "postgres" || formDatabase() === "") setFormDatabase("mysql");
                    }}
                    style={`display: flex; align-items: center; gap: 14px; padding: 12px 18px; border-radius: 10px; border: 2px solid transparent; background: ${formType() === "mysql" ? "rgba(255, 140, 0, 0.06)" : "rgba(255, 255, 255, 0.015)"}; cursor: pointer; transition: all 0.2s ease-in-out; box-shadow: ${formType() === "mysql" ? "0 4px 12px rgba(255, 140, 0, 0.1)" : "none"}`}
                    class="db-type-card"
                  >
                    <div style="display: flex; align-items: center; justify-content: center; width: 34px; height: 34px; background: rgba(255, 140, 0, 0.12); color: #ffa500; border-radius: 8px; flex-shrink: 0;">
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="width: 18px; height: 18px;">
                        <ellipse cx="12" cy="5" rx="9" ry="3"></ellipse>
                        <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5"></path>
                        <path d="M3 12c0 1.66 4 3 9 3s9-1.34 9-3"></path>
                      </svg>
                    </div>
                    <div style="flex: 1; min-width: 0;">
                      <div style="font-weight: 600; font-size: 13.5px; color: var(--text-primary);">MySQL</div>
                      <div style="font-size: 11px; color: var(--text-dim); margin-top: 2px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">{t("dbTypeMysqlDesc")}</div>
                    </div>
                    <span 
                      style={`color: #ffa500; font-size: 15px; font-weight: bold; margin-left: auto; transition: opacity 0.15s ease-in-out; ${
                        formType() === "mysql" ? "opacity: 1;" : "opacity: 0; pointer-events: none;"
                      }`}
                    >
                      ✓
                    </span>
                  </div>

                  {/* SQLite Card */}
                  <div 
                    onClick={() => {
                      setFormType("sqlite");
                      setFormPort(0);
                      setFormHost("");
                      setFormUsername("");
                      setFormPassword("");
                      setFormSslMode("disable");
                      if (formName() === "local_postgres" || formName() === "local_mysql" || formName() === "") setFormName("local_sqlite");
                      if (formDatabase() === "postgres" || formDatabase() === "mysql") setFormDatabase("");
                    }}
                    style={`display: flex; align-items: center; gap: 14px; padding: 12px 18px; border-radius: 10px; border: 2px solid transparent; background: ${formType() === "sqlite" ? "rgba(16, 185, 129, 0.06)" : "rgba(255, 255, 255, 0.015)"}; cursor: pointer; transition: all 0.2s ease-in-out; box-shadow: ${formType() === "sqlite" ? "0 4px 12px rgba(16, 185, 129, 0.1)" : "none"}`}
                    class="db-type-card"
                  >
                    <div style="display: flex; align-items: center; justify-content: center; width: 34px; height: 34px; background: rgba(16, 185, 129, 0.12); color: #10b981; border-radius: 8px; flex-shrink: 0;">
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="width: 18px; height: 18px;">
                        <ellipse cx="12" cy="5" rx="9" ry="3"></ellipse>
                        <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5"></path>
                        <path d="M3 12c0 1.66 4 3 9 3s9-1.34 9-3"></path>
                      </svg>
                    </div>
                    <div style="flex: 1; min-width: 0;">
                      <div style="font-weight: 600; font-size: 13.5px; color: var(--text-primary);">SQLite</div>
                      <div style="font-size: 11px; color: var(--text-dim); margin-top: 2px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">{t("dbTypeSqliteDesc")}</div>
                    </div>
                    <span 
                      style={`color: #10b981; font-size: 15px; font-weight: bold; margin-left: auto; transition: opacity 0.15s ease-in-out; ${
                        formType() === "sqlite" ? "opacity: 1;" : "opacity: 0; pointer-events: none;"
                      }`}
                    >
                      ✓
                    </span>
                  </div>
                </div>
              </div>

              <Show when={formType() !== "sqlite"}>
              {/* Group 1: Connection & Network */}
              <div style="display: flex; flex-direction: column; gap: 14px;">
                <div style="font-size: 12.5px; font-weight: 600; color: var(--text-primary); border-left: 3px solid var(--brand); padding-left: 8px; margin-bottom: 4px;">{t("connNetworkConfig")}</div>
                
                <div style="display: grid; grid-template-columns: 2fr 3fr 1fr; gap: 12px;">
                  <div style="display: flex; flex-direction: column; gap: 6px;">
                    <label style="font-size: 11.5px; color: var(--text-dim);">{t("connNameLabel")}</label>
                    <input
                      type="text"
                      class="sp-input"
                      value={formName()}
                      placeholder="neon_prod"
                      onInput={(e) => setFormName(e.currentTarget.value)}
                    />
                  </div>

                  <div style="display: flex; flex-direction: column; gap: 6px;">
                    <label style="font-size: 11.5px; color: var(--text-dim);">{t("hostLabel")}</label>
                    <input
                      type="text"
                      class="sp-input"
                      value={formHost()}
                      placeholder="Host / IP"
                      onInput={(e) => setFormHost(e.currentTarget.value)}
                    />
                  </div>

                  <div style="display: flex; flex-direction: column; gap: 6px;">
                    <label style="font-size: 11.5px; color: var(--text-dim);">{t("portLabel")}</label>
                    <input
                      type="number"
                      class="sp-input"
                      value={formPort()}
                      onInput={(e) => setFormPort(parseInt(e.currentTarget.value) || 0)}
                    />
                  </div>
                </div>
              </div>

              {/* Group 2: Auth & DB info */}
              <div style="display: flex; flex-direction: column; gap: 14px; min-height: 166px; border-top: 1px solid var(--border-faint); padding-top: 20px; margin-top: 6px;">
                <div style="font-size: 12.5px; font-weight: 600; color: var(--text-primary); border-left: 3px solid var(--brand); padding-left: 8px; margin-bottom: 4px;">{t("authPermissions")}</div>

                <div style="display: grid; grid-template-columns: 1fr 1fr; gap: 14px;">
                  <div style="display: flex; flex-direction: column; gap: 6px;">
                    <label style="font-size: 11.5px; color: var(--text-dim);">{t("databaseLabel")}</label>
                    <input
                      type="text"
                      class="sp-input"
                      value={formDatabase()}
                      placeholder="neondb"
                      onInput={(e) => setFormDatabase(e.currentTarget.value)}
                    />
                  </div>

                  <div style="display: flex; flex-direction: column; gap: 6px;">
                    <label style="font-size: 11.5px; color: var(--text-dim);">{t("usernameLabel")}</label>
                    <input
                      type="text"
                      class="sp-input"
                      value={formUsername()}
                      placeholder="Username"
                      onInput={(e) => setFormUsername(e.currentTarget.value)}
                    />
                  </div>
                </div>

                <div style="display: grid; grid-template-columns: 1fr 1fr; gap: 14px; align-items: flex-end;">
                  <div style="display: flex; flex-direction: column; gap: 6px;">
                    <label style="font-size: 11.5px; color: var(--text-dim);">{t("passwordLabel")}</label>
                    <div style="position: relative; display: flex; align-items: center; width: 100%;">
                      <input
                        type={showPassword() ? "text" : "password"}
                        class="sp-input"
                        style="padding-right: 40px; width: 100%;"
                        value={formPassword()}
                        placeholder="••••••••"
                        onInput={(e) => setFormPassword(e.currentTarget.value)}
                      />
                      <button 
                        type="button" 
                        onClick={() => setShowPassword(!showPassword())}
                        style="position: absolute; right: 8px; background: transparent; border: none; cursor: pointer; color: var(--text-dim); display: flex; align-items: center; justify-content: center; padding: 6px;"
                      >
                        <Show when={showPassword()} fallback={
                          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 15px; height: 15px; opacity: 0.6;">
                            <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/>
                            <circle cx="12" cy="12" r="3"/>
                          </svg>
                        }>
                          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 15px; height: 15px; opacity: 0.6;">
                            <path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24"/>
                            <line x1="1" y1="1" x2="23" y2="23"/>
                          </svg>
                        </Show>
                      </button>
                    </div>
                  </div>

                  {/* SSL Mode (Postgres Only) */}
                  <Show when={formType() === "postgres"}>
                    <div style="display: flex; flex-direction: column; gap: 6px;">
                      <div style="display: flex; align-items: center;">
                        <label style="font-size: 11.5px; color: var(--text-dim);">{t("sslModeLabel")}</label>
                      </div>
                      <Select
                        options={[
                          { label: "disable", value: "disable" },
                          { label: "require", value: "require" },
                          { label: "verify-ca", value: "verify-ca" },
                          { label: "verify-full", value: "verify-full" }
                        ]}
                        value={formSslMode()}
                        onChange={(v) => setFormSslMode(v)}
                        width="100%"
                      />
                    </div>
                  </Show>
                </div>
              </div>
              </Show>

              {/* SQLite: file path picker (shown only for sqlite) */}
              <Show when={formType() === "sqlite"}>
                <div style="display: flex; flex-direction: column; gap: 14px; min-height: 166px; border-top: 1px solid var(--border-faint); padding-top: 20px; margin-top: 6px;">
                  <div style="font-size: 12.5px; font-weight: 600; color: var(--text-primary); border-left: 3px solid var(--brand); padding-left: 8px; margin-bottom: 4px;">{t("connNetworkConfig")}</div>
                  <div style="display: flex; flex-direction: column; gap: 6px;">
                    <label style="font-size: 11.5px; color: var(--text-dim);">{t("connNameLabel")}</label>
                    <input
                      type="text"
                      class="sp-input"
                      value={formName()}
                      placeholder="local_sqlite"
                      onInput={(e) => setFormName(e.currentTarget.value)}
                    />
                  </div>
                  <div style="display: flex; flex-direction: column; gap: 6px;">
                    <label style="font-size: 11.5px; color: var(--text-dim);">{t("filePathLabel")}</label>
                    <div style="display: flex; gap: 8px; align-items: stretch;">
                      <input
                        type="text"
                        class="sp-input"
                        style="flex: 1; min-width: 0;"
                        value={formDatabase()}
                        placeholder="/path/to/database.db"
                        onInput={(e) => setFormDatabase(e.currentTarget.value)}
                      />
                      <button
                        type="button"
                        class="ss-btn ss-btn-secondary btn-sec-no-highlight"
                        style="padding: 6px 14px; font-size: 13px; flex-shrink: 0;"
                        onClick={async () => {
                          const p = await selectFile();
                          if (p) setFormDatabase(p);
                        }}
                      >
                        {t("browseFileBtn")}
                      </button>
                    </div>
                  </div>
                </div>
              </Show>

              {/* Bottom Test & Action Footer */}
              <div style="display: flex; flex-direction: column; gap: 12px; margin-top: 6px; padding-top: 18px; border-top: 1px solid var(--border-faint);">
                <div style="display: flex; align-items: center; gap: 12px;">
                  <button 
                    class="ss-btn ss-btn-secondary btn-sec-no-highlight" 
                    style="padding: 6px 16px; font-size: 13px; display: flex; align-items: center; gap: 6px;"
                    onClick={handleTestConnection} 
                    disabled={testStatus().status === "testing"}
                  >
                    <Show when={testStatus().status === "testing"}>
                      <div class="loader-spinner" style="border-top-color: var(--text-primary); width: 11px; height: 11px; border-width: 1.5px;"></div>
                    </Show>
                    {testStatus().status === "testing" ? "正在测试..." : t("testConnBtn")}
                  </button>

                  {/* Status Indicator inline next to the button */}
                  <Show when={testStatus().status === "success"}>
                    <span style="font-size: 12.5px; color: var(--text-success); display: flex; align-items: center; gap: 4px; font-weight: 500;">
                      ✓ {t("testConnSuccess")}
                    </span>
                  </Show>
                  <Show when={testStatus().status === "error"}>
                    <div style="display: flex; align-items: center; gap: 6px; max-width: 340px; min-width: 0;">
                      <span 
                        style="font-size: 12px; color: var(--text-danger); display: flex; align-items: center; gap: 4px; font-weight: 500; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; flex: 1; min-width: 0;" 
                        title={testStatus().msg}
                      >
                        ✕ {testStatus().msg}
                      </span>
                      <button
                        title={copiedTestError() ? "已复制" : "复制错误日志"}
                        style="border: none; background: transparent; color: var(--text-dim); cursor: pointer; padding: 2px; display: inline-flex; align-items: center; justify-content: center; border-radius: 4px; transition: all 0.15s ease; flex-shrink: 0;"
                        onClick={async (e) => {
                          e.stopPropagation();
                          try {
                            await navigator.clipboard.writeText(testStatus().msg || "");
                            setCopiedTestError(true);
                            setTimeout(() => setCopiedTestError(false), 1500);
                          } catch {}
                        }}
                        onMouseEnter={(e) => {
                          e.currentTarget.style.color = "var(--text-primary)";
                          e.currentTarget.style.background = "var(--bg-hover)";
                        }}
                        onMouseLeave={(e) => {
                          e.currentTarget.style.color = "var(--text-dim)";
                          e.currentTarget.style.background = "transparent";
                        }}
                      >
                        <Show
                          when={copiedTestError()}
                          fallback={
                            <svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                              <rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect>
                              <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path>
                            </svg>
                          }
                        >
                          <svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="var(--accent-green, #10b981)" stroke-width="3" stroke-linecap="round" stroke-linejoin="round">
                            <polyline points="20 6 9 17 4 12"></polyline>
                          </svg>
                        </Show>
                      </button>
                    </div>
                  </Show>

                  <button 
                    class="ss-btn btn-prim-no-highlight" 
                    style="padding: 6px 20px; font-size: 13px; font-weight: 500; margin-left: auto;"
                    onClick={handleSaveConnection}
                  >
                    {t("saveAndApplyBtn")}
                  </button>
                </div>
              </div>
            </div>
          }>
            <div style="margin-top: 8px; display: flex; flex-direction: column; gap: 14px; max-width: 680px;">

              <Show when={connections().length > 0} fallback={
                <div style="padding: 48px 24px; text-align: center; color: var(--text-dim); border: 1px dashed var(--border-strong); border-radius: 10px; background: rgba(255,255,255,0.015); font-style: italic; font-size: 12.5px; display: flex; flex-direction: column; align-items: center; gap: 8px;">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" style="width: 32px; height: 32px; opacity: 0.5;">
                    <ellipse cx="12" cy="5" rx="9" ry="3"></ellipse>
                    <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5"></path>
                    <path d="M3 12c0 1.66 4 3 9 3s9-1.34 9-3"></path>
                  </svg>
                  <span>{t("noConnectionsDesc")}</span>
                </div>
              }>
                <div style="display: flex; flex-direction: column; gap: 10px;">
                  <For each={connections()}>
                    {(c) => (
                      <div 
                        style="display: flex; justify-content: space-between; align-items: center; padding: 14px 20px; border: 1px solid var(--border-strong); border-radius: 10px; background: rgba(255,255,255,0.015); transition: transform 0.2s ease, background 0.2s ease; hover: background: rgba(255,255,255,0.03);"
                        class="db-connection-item-row"
                      >
                        <div style="display: flex; align-items: center; gap: 14px;">
                          <span style={`display: inline-flex; align-items: center; justify-content: center; width: 36px; height: 36px; background: ${c.dbType === 'postgres' ? 'rgba(80, 160, 255, 0.12)' : c.dbType === 'sqlite' ? 'rgba(16, 185, 129, 0.12)' : 'rgba(255, 140, 0, 0.12)'}; color: ${c.dbType === 'postgres' ? 'var(--brand)' : c.dbType === 'sqlite' ? 'var(--accent-green)' : 'var(--accent-amber)'}; border-radius: 8px; flex-shrink: 0;`}>
                            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="width: 18px; height: 18px;">
                              <ellipse cx="12" cy="5" rx="9" ry="3"></ellipse>
                              <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5"></path>
                              <path d="M3 12c0 1.66 4 3 9 3s9-1.34 9-3"></path>
                            </svg>
                          </span>
                          <div>
                            <div style="display: flex; align-items: center; gap: 8px;">
                              <span style="font-weight: 600; font-size: 14px; color: var(--text-primary);">{c.name}</span>
                              <span style={`font-size: 10px; font-weight: bold; text-transform: uppercase; padding: 1px 6px; border-radius: 4px; background: ${c.dbType === 'postgres' ? 'rgba(80, 160, 255, 0.12)' : c.dbType === 'sqlite' ? 'rgba(16, 185, 129, 0.12)' : 'rgba(255, 140, 0, 0.12)'}; color: ${c.dbType === 'postgres' ? 'var(--brand)' : c.dbType === 'sqlite' ? 'var(--accent-green)' : 'var(--accent-amber)'}`}>{c.dbType}</span>
                            </div>
                            <div style="font-size: 12px; color: var(--text-dim); margin-top: 4px; font-family: var(--font-mono, monospace); word-break: break-all;">
                              {c.dbType === 'sqlite' ? c.databaseName : `${c.username}@${c.host}:${c.port}/${c.databaseName}`}
                            </div>
                          </div>
                        </div>
                        <div style="display: flex; gap: 8px; align-items: center; flex-shrink: 0; margin-left: 12px;">
                          <Show when={props.workspacePath}>
                            <button 
                              class="db-icon-btn" 
                              classList={{ active: !!linkedConns()[c.id] }}
                              onClick={() => handleToggleLink(c.id)}
                              title={linkedConns()[c.id] ? t("unlinkFromWorkspaceTooltip") : t("linkToWorkspaceTooltip")}
                            >
                              <Show when={linkedConns()[c.id]} fallback={
                                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 15px; height: 15px; opacity: 0.65;">
                                  <rect x="2" y="6" width="20" height="12" rx="6" ry="6"></rect>
                                  <circle cx="8" cy="12" r="2"></circle>
                                </svg>
                              }>
                                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 15px; height: 15px;">
                                  <rect x="2" y="6" width="20" height="12" rx="6" ry="6" fill="currentColor" fill-opacity="0.25"></rect>
                                  <circle cx="16" cy="12" r="2" fill="currentColor"></circle>
                                </svg>
                              </Show>
                            </button>
                          </Show>
                          <button 
                            class="db-icon-btn" 
                            onClick={() => startEditConnection(c)}
                            title="编辑"
                          >
                            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px; opacity: 0.85;">
                              <path d="M12 20h9"></path>
                              <path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5z"></path>
                            </svg>
                          </button>
                          <button 
                            class="db-icon-btn btn-delete" 
                            onClick={() => handleDeleteConnection(c.id)}
                            title="删除"
                          >
                            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
                              <polyline points="3 6 5 6 21 6"></polyline>
                              <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"></path>
                            </svg>
                          </button>
                        </div>
                      </div>
                    )}
                  </For>
                </div>
              </Show>
            </div>
          </Show>
        </Show>

        {/* Tab 1: General Settings (常规) */}
        <Show when={activeTab() === "general"}>
          <div class="settings-view-header">
            <h2>{t("settingsGeneral")}</h2>
            <p class="settings-view-subtitle">{t("generalSettingsDesc")}</p>
          </div>
          
          <div class="settings-section-card">
            <div class="settings-row-control">
              <div class="settings-row-info">
                <span class="label-title">界面主题</span>
                <p class="settings-row-desc">切换应用界面使用的主题外观。</p>
              </div>
              <Select
                value={currentTheme()}
                onChange={(v) => setCurrentTheme(v as any)}
                width="fit-content"
                options={[
                  { value: "geek-dark", label: "极客暗黑", icon: <MoonIcon /> },
                  { value: "classic-dark", label: "经典深色", icon: <MoonIcon /> },
                  { value: "light", label: "极致浅色", icon: <SunIcon /> },
                ]}
              />
            </div>

            <div class="settings-row-control">
              <div class="settings-row-info">
                <span class="label-title">界面语言</span>
                <p class="settings-row-desc">选择应用 UI 的显示语言。</p>
              </div>
              <Select
                value={currentLanguage()}
                onChange={(v) => setCurrentLanguage(v as any)}
                width="fit-content"
                options={[
                  { value: "zh", label: "简体中文" },
                  { value: "en", label: "English" },
                ]}
              />
            </div>

            <div class="settings-row-control">
              <div class="settings-row-info">
                <span class="label-title">界面缩放</span>
                <p class="settings-row-desc">调整当前窗口中文本和控件的整体显示大小。</p>
              </div>
              <div class="segmented-control">
                <button 
                  classList={{ active: currentZoom() === 90 }} 
                  onClick={() => setCurrentZoom(90)}
                >
                  偏小
                </button>
                <button 
                  classList={{ active: currentZoom() === 100 }} 
                  onClick={() => setCurrentZoom(100)}
                >
                  正常
                </button>
                <button 
                  classList={{ active: currentZoom() === 110 }} 
                  onClick={() => setCurrentZoom(110)}
                >
                  偏大
                </button>
              </div>
            </div>

            <div class="settings-row-control">
              <div class="settings-row-info">
                <span class="label-title">查询超时限制</span>
                <p class="settings-row-desc">即席执行 SQL 或对话探索时单次查询的最大允许时间，超时后发送中断信号。</p>
              </div>
              <Select
                value={String(settings().queryTimeout !== undefined ? settings().queryTimeout : 60)}
                onChange={(v) => updateSetting("queryTimeout", Number(v))}
                width="fit-content"
                options={[
                  { value: "30", label: "30 秒" },
                  { value: "60", label: "1 分钟 (默认)" },
                  { value: "180", label: "3 分钟" },
                  { value: "300", label: "5 分钟" },
                  { value: "0", label: "无限制" },
                ]}
              />
            </div>
            <div class="settings-row-control">
              <div class="settings-row-info">
                <span class="label-title">最大等待时间</span>
                <p class="settings-row-desc">当查询超时后数据库引擎未能及时响应中断时，此限制确保请求在指定时间内必定返回，避免长时间卡顿。</p>
              </div>
              <Select
                value={String(settings().queryHardTimeout !== undefined ? settings().queryHardTimeout : "auto")}
                onChange={(v) => updateSetting("queryHardTimeout", v === "auto" ? undefined : Number(v))}
                width="fit-content"
                options={[
                  { value: "auto", label: `自动（超时 ×2 = ${((settings().queryTimeout ?? 60) || 0) * 2} 秒）` },
                  { value: "60", label: "1 分钟" },
                  { value: "120", label: "2 分钟" },
                  { value: "180", label: "3 分钟" },
                  { value: "300", label: "5 分钟" },
                  { value: "600", label: "10 分钟" },
                  { value: "0", label: "无限制" },
                ]}
              />
            </div>
            <div class="settings-row-control">
              <div class="settings-row-info">
                <span class="label-title">外部数据库物化采样</span>
                <p class="settings-row-desc">引入外部数据库表（PostgreSQL/MySQL/SQLite）时开启本地物化采样缓存，以大幅提升探索和字段结构查验速度，并保护生产数据库。</p>
              </div>
              <button
                class="ss-toggle"
                classList={{ on: sampleEnabled() }}
                onClick={() => handleToggleSampleEnabled(!sampleEnabled())}
                aria-label="外部数据库物化采样"
              />
            </div>

            <div class="settings-row-control">
              <div class="settings-row-info">
                <span class="label-title">物化采样限制行数</span>
                <p class="settings-row-desc">进行外部表本地物化采样缓存时，每个表的最大采样行数限制。</p>
              </div>
              <input
                type="number"
                class="sp-input"
                style={{ width: "120px", "text-align": "right" }}
                value={sampleLimit()}
                onInput={(e) => handleUpdateSampleLimit(parseInt(e.currentTarget.value) || 0)}
                disabled={!sampleEnabled()}
                min="1"
              />
            </div>
          </div>
        </Show>

        {/* Tab: Commands (命令) — 快捷键 + AI 数据工具的透明清单。
            命令集相对固定，采用前端静态清单；新增命令/工具时需同步更新下表。 */}
        <Show when={activeTab() === "commands"}>
          <div class="settings-view-header">
            <h2>{t("settingsCommands")}</h2>
            <p class="settings-view-subtitle">查看应用支持的快捷键与 AI 数据工具。</p>
          </div>

          {/* 分类一：快捷键 */}
          <div class="settings-section-card">
            <div class="settings-card-title">快捷键</div>
            <table class="cmd-table">
              <tbody>
                <tr><td class="cmd-key">⌘ N</td><td class="cmd-desc">新建查询</td></tr>
                <tr><td class="cmd-key">⇧ ⌘ N</td><td class="cmd-desc">新建对话</td></tr>
                <tr><td class="cmd-key">⌘ S</td><td class="cmd-desc">保存当前查询</td></tr>
              </tbody>
            </table>
          </div>

          {/* 分类二：AI 数据工具（对话模式下 Agent 可调用的工具）。
              第一列同时展示对话中看到的中文标签和后端工具名，让用户能对应起来。 */}
          <div class="settings-section-card">
            <div class="settings-card-title">AI 数据工具</div>
            <table class="cmd-table">
              <tbody>
                <tr>
                  <td class="cmd-key"><span class="cmd-label">探索数据库</span><span class="cmd-name">list_tables</span></td>
                  <td class="cmd-desc">列出当前数据库中的所有数据表和视图名。</td>
                </tr>
                <tr>
                  <td class="cmd-key"><span class="cmd-label">分析表结构</span><span class="cmd-name">describe_table</span></td>
                  <td class="cmd-desc">获取指定表或视图的结构（列名、数据类型等）。</td>
                </tr>
                <tr>
                  <td class="cmd-key"><span class="cmd-label">采样数据</span><span class="cmd-name">sample_data</span></td>
                  <td class="cmd-desc">获取指定表或视图的前 5 行样例数据。</td>
                </tr>
                <tr>
                  <td class="cmd-key"><span class="cmd-label">执行 SQL</span><span class="cmd-name">execute_query</span></td>
                  <td class="cmd-desc">执行只读的 SQL 查询，并返回结果。</td>
                </tr>
                <tr>
                  <td class="cmd-key"><span class="cmd-label">创建表</span><span class="cmd-name">create_table</span></td>
                  <td class="cmd-desc">创建物化物理表持久化加工后的数据（t_/tmp_ 前缀）。</td>
                </tr>
                <tr>
                  <td class="cmd-key"><span class="cmd-label">创建视图</span><span class="cmd-name">create_view</span></td>
                  <td class="cmd-desc">创建零拷贝虚拟视图封装查询逻辑（v_/tmp_v_ 前缀）。</td>
                </tr>
                <tr>
                  <td class="cmd-key"><span class="cmd-label">删除对象</span><span class="cmd-name">drop_object</span></td>
                  <td class="cmd-desc">删除指定的表或视图，有下游依赖时自动级联删除。</td>
                </tr>
                <tr>
                  <td class="cmd-key"><span class="cmd-label">生成图表</span><span class="cmd-name">render_chart</span></td>
                  <td class="cmd-desc">用图表可视化查询结果（柱状/折线/饼图/散点/漏斗/仪表盘），基础类型可切换。</td>
                </tr>
              </tbody>
            </table>
          </div>
        </Show>

        {/* Tab: Code Preview (代码预览) — 代码块语法高亮配置，全部立即生效。 */}
        <Show when={activeTab() === "codePreview"}>
          <div class="settings-view-header">
            <h2>{t("settingsCodePreview")}</h2>
            <p class="settings-view-subtitle">调整代码块的显示样式，配色随界面主题自动切换。</p>
          </div>

          <div class="settings-section-card">
            <div class="settings-card-title">显示</div>

            <div class="settings-row-control">
              <div class="settings-row-info">
                <span class="label-title">显示行号</span>
                <p class="settings-row-desc">在每行代码前标注序号，便于定位。</p>
              </div>
              <button
                class="ss-toggle"
                classList={{ on: codeLineNumbers() }}
                onClick={() => setCodeLineNumbersP(!codeLineNumbers())}
                aria-label="显示行号"
              />
            </div>

            <div class="settings-row-control">
              <div class="settings-row-info">
                <span class="label-title">长行自动换行</span>
                <p class="settings-row-desc">超出宽度的代码自动折行显示，避免横向滚动。</p>
              </div>
              <button
                class="ss-toggle"
                classList={{ on: codeWrap() }}
                onClick={() => setCodeWrapP(!codeWrap())}
                aria-label="长行自动换行"
              />
            </div>

            <div class="settings-row-control">
              <div class="settings-row-info">
                <span class="label-title">代码字号</span>
                <p class="settings-row-desc">代码块的字号大小，单位为像素。</p>
              </div>
              <Select
                value={String(codeFontSize())}
                onChange={(v) => setCodeFontSizeP(parseInt(v))}
                width="80px"
                options={[
                  { value: "12", label: "12" },
                  { value: "13", label: "13" },
                  { value: "14", label: "14" },
                  { value: "16", label: "16" },
                ]}
              />
            </div>
          </div>
        </Show>

        {/* Tab 2: Model Settings (模型设置) - Premium High-Fidelity Details */}
        <Show when={activeTab() === "modelSettings"}>
          <div class="settings-view-header wide-header">
            <h2>{t("modelSettings")}</h2>
            <p class="settings-view-subtitle">{t("settingsSubtitle")}</p>
          </div>

          <div class="settings-panel-box two-cols">
            
            {/* Left Column: Providers List */}
            <div class="sp-left-panel">
              <div class="sp-section-lbl">模型供应商</div>
              
              <For each={settings().providers}>
                {(prov) => (
                  <button
                    class="sp-provider-item"
                    classList={{ active: selectedProvider() === prov.id && !isAddingProvider() }}
                    onClick={() => { setSelectedProvider(prov.id); setIsAddingProvider(false); }}
                  >
                    <span
                      class="provider-dot"
                      classList={{
                        // Four states, mutually exclusive. An ENABLED provider
                        // always shows the green active dot (persists across
                        // restarts, never gated). A disabled provider reflects
                        // the in-session batch test: grey (untested/default),
                        // blue (all models validated → ready to enable), or red
                        // (any model failed). Green is reserved for "enabled" so
                        // a validated-but-not-yet-enabled provider isn't
                        // mistaken for enabled.
                        active: prov.enabled && !!prov.apiKey,
                        "test-success": !prov.enabled && providerValidated(prov.id, prov.models || []),
                        "test-fail": !prov.enabled && (prov.models || []).some(m => modelTests()[prov.id]?.[m.id]?.status === "error") && !providerValidated(prov.id, prov.models || []),
                      }}
                    />
                    <span class="provider-name">{prov.name}</span>
                  </button>
                )}
              </For>

              <button class="sp-add-btn" onClick={() => { setIsAddingProvider(true); setSelectedProvider(""); }}>
                <span class="add-icon">+</span>
                <span>添加供应商</span>
              </button>
            </div>

            {/* Right Column: Provider Details */}
            <div class="sp-right-panel">
              <Show
                when={!isAddingProvider()}
                fallback={
                  <>
                      <div class="provider-detail-header">
                        <div class="pd-title-group">
                          <h3>添加模型供应商</h3>
                        </div>
                      </div>
                    <p class="settings-view-desc" style="margin-top: -10px; margin-bottom: 20px; color: var(--text-dim); font-size: 12px;">
                      配置一个完全自定义的 API 端点和初始模型。
                    </p>

                    <div class="sp-form-section" style="margin-top: 0;">
                      <div class="sp-form-row">
                        <span class="sp-form-label">名称</span>
                        <input 
                          type="text" 
                          class="sp-input" 
                          placeholder="如：智谱 GLM"
                          value={newProviderName()}
                          onInput={(e) => setNewProviderName(e.currentTarget.value)}
                        />
                      </div>

                      <div class="sp-form-row">
                        <span class="sp-form-label">Base URL</span>
                        <input 
                          type="text" 
                          class="sp-input" 
                          placeholder="https://api.example.com/v1"
                          value={newProviderEndpoint()}
                          onInput={(e) => { setNewProviderEndpoint(e.currentTarget.value); clearModelTests(NEW_PROVIDER_TEST_KEY); }}
                        />
                      </div>

                      <div class="sp-form-row">
                        <span class="sp-form-label">API Key</span>
                        <div class="sp-input-wrapper">
                          <input 
                            type={showApiKey() ? "text" : "password"} 
                            class="sp-input password-input" 
                            placeholder="输入 API Key"
                            value={newProviderApiKey()}
                            onInput={(e) => { setNewProviderApiKey(e.currentTarget.value); clearModelTests(NEW_PROVIDER_TEST_KEY); }}
                          />
                          <button class="sp-pwd-toggle" onClick={() => setShowApiKey(!showApiKey())}>
                            {showApiKey() ? (
                              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
                                <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"></path>
                                <circle cx="12" cy="12" r="3"></circle>
                              </svg>
                            ) : (
                              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
                                <path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24"></path>
                                <line x1="1" y1="1" x2="23" y2="23"></line>
                              </svg>
                            )}
                          </button>
                        </div>
                      </div>

                      <div class="sp-form-row">
                        <span class="sp-form-label">API 格式</span>
                          <Select
                            value={newProviderFormat()}
                            onChange={(v) => { setNewProviderFormat(v as any); clearModelTests(NEW_PROVIDER_TEST_KEY); }}
                            width="100%"
                            options={[
                              { value: "anthropic", label: "Anthropic Messages (/v1/messages)" },
                              { value: "openai", label: "Chat Completions (/chat/completions)" },
                              { value: "responses", label: "Responses (/responses)" },
                            ]}
                          />
                      </div>

                      <div class="sp-form-row" style="margin-top: 10px;">
                        <span class="sp-form-label">模型列表</span>
                        <div class="models-list">
                          <For each={newProviderModels()}>
                            {(model) => (
                              <div class="model-row">
                                <span class="model-name-lbl">{model.id}</span>
                                <div style="display: flex; align-items: center; gap: 12px;">
                                  <span class="sp-context-badge" title="上下文窗口">
                                    {model.contextWindow >= 10000 ? `${model.contextWindow / 10000}万` : model.contextWindow}
                                  </span>
                                  <span class="sp-context-badge" title="最大输出 Token">
                                    Out: {model.maxTokens || 4096}
                                  </span>
                                  {/* Per-model test status (new-provider form, in-memory). */}
                                  <ModelStatusIcon
                                    status={modelTests()[NEW_PROVIDER_TEST_KEY]?.[model.id]?.status}
                                    errorTip={modelTests()[NEW_PROVIDER_TEST_KEY]?.[model.id]?.msg}
                                  />
                                  <div class="model-actions-btns" style="display: flex; align-items: center; gap: 8px;">
                                    <button class="sp-action-icon-btn" title="编辑模型" onClick={() => handleOpenEditTempModel(model)}>
                                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 13px; height: 13px;">
                                        <path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"></path>
                                        <path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"></path>
                                      </svg>
                                    </button>
                                    <button class="sp-action-icon-btn" title="删除模型" onClick={() => { setNewProviderModels(newProviderModels().filter(m => m.id !== model.id)); clearModelTests(NEW_PROVIDER_TEST_KEY); }}>
                                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 13px; height: 13px;">
                                        <polyline points="3 6 5 6 21 6"></polyline>
                                        <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"></path>
                                      </svg>
                                    </button>
                                  </div>
                                </div>
                              </div>
                            )}
                          </For>
                          <button class="sp-add-model-inline-btn" onClick={handleOpenAddTempModel}>
                            + 添加模型
                          </button>
                        </div>
                      </div>
                    </div>

                    {/* Batch connection test for the new-provider form — tests
                        every model before saving. Keyed by the new-provider
                        sentinel so its state is isolated. */}
                    <div style="display: flex; flex-direction: column; gap: 12px; margin-top: 6px; padding-top: 18px; border-top: 1px solid var(--border-faint);">
                      <div style="display: flex; align-items: center; gap: 12px; flex-wrap: wrap;">
                        <button
                          class="ss-btn ss-btn-secondary btn-sec-no-highlight"
                          style="padding: 6px 16px; font-size: 13px; display: flex; align-items: center; gap: 6px;"
                          onClick={handleTestNewProviderConnection}
                          disabled={batchTesting(NEW_PROVIDER_TEST_KEY)}
                        >
                          <Show when={batchTesting(NEW_PROVIDER_TEST_KEY)}>
                            <div class="loader-spinner" style="border-top-color: var(--text-primary); width: 11px; height: 11px; border-width: 1.5px;"></div>
                          </Show>
                          {batchTesting(NEW_PROVIDER_TEST_KEY) ? t("modelTesting") : t("modelTestBtn")}
                        </button>
                      </div>
                    </div>

                    <div style="margin-top: 20px; display: flex; gap: 12px;">
                      <button class="sp-btn-primary" onClick={handleCreateNewProvider}>
                        添加供应商
                      </button>
                      <button class="sp-btn-secondary" onClick={() => setIsAddingProvider(false)}>
                        取消
                      </button>
                    </div>
                  </>
                }
              >
                {(() => {
                  const prov = (settings().providers || []).find(p => p.id === selectedProvider());
                  if (!prov) {
                    return (
                      <div class="sp-empty-provider" style="display: flex; flex-direction: column; align-items: center; justify-content: center; height: 100%; color: var(--text-dim); text-align: center; padding: 40px;">
                        <div style="width: 56px; height: 56px; border-radius: 14px; background: var(--bg-elevated); border: 1px solid var(--border-strong); display: flex; align-items: center; justify-content: center; margin-bottom: 16px;">
                          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" style="width: 28px; height: 28px;">
                            <path d="M12 22v-5" />
                            <path d="M9 8V2" />
                            <path d="M15 8V2" />
                            <path d="M18 8v5a4 4 0 0 1-4 4h-4a4 4 0 0 1-4-4V8Z" />
                          </svg>
                        </div>
                        <h4 style="color: var(--text); font-weight: 500;">暂无模型供应商</h4>
                        <p style="font-size: 12px; margin-top: 6px; max-width: 280px; line-height: 1.5;">点击左侧「添加供应商」按钮配置您的 AI API 接口和模型</p>
                      </div>
                    );
                  }

                  return (
                    <>
                      <div class="provider-detail-header" style="border-bottom: none; padding-bottom: 0; margin-bottom: 12px;">
                        <div class="pd-title-group">
                          <Show
                            when={editingProviderId() === prov.id}
                            fallback={
                              <div style="display: flex; align-items: center; gap: 6px;">
                                <h3>{prov.name}</h3>
                                <button class="sp-edit-title-btn" title="编辑名称" onClick={() => {
                                  setEditingProviderId(prov.id);
                                  setTempName(prov.name);
                                }}>
                                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 13px; height: 13px;">
                                    <path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"></path>
                                    <path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"></path>
                                  </svg>
                                </button>
                              </div>
                            }
                          >
                            <div style="display: flex; align-items: center; gap: 6px;">
                              <input 
                                type="text" 
                                class="sp-input" 
                                style="width: 150px; height: 26px; padding: 2px 8px;"
                                value={tempName()}
                                onInput={(e) => setTempName(e.currentTarget.value)}
                                onBlur={handleSaveProviderName}
                                onKeyDown={(e) => {
                                  if (e.key === "Enter") handleSaveProviderName();
                                  if (e.key === "Escape") setEditingProviderId(null);
                                }}
                                autofocus
                              />
                            </div>
                          </Show>

                          <div class="sp-status-btn-group">
                            {(() => {
                              // Enable gate: a disabled provider can only be
                              // switched ON once all its models are validated
                              // available in this session. Already-enabled
                              // providers are never gated (they persist enabled
                              // across restarts).
                              const canEnable = prov.enabled || providerValidated(prov.id, prov.models || []);
                              return (
                                <button
                                  class={`status-btn enabled-btn ${prov.enabled ? "active" : ""}`}
                                  disabled={!canEnable}
                                  title={canEnable ? "" : t("modelEnableGateTip")}
                                  style={canEnable ? {} : { opacity: 0.45, cursor: "not-allowed" }}
                                  onClick={() => {
                                    if (canEnable) updateProviderProperty(prov.id, "enabled", true);
                                  }}
                                >
                                  已启用
                                </button>
                              );
                            })()}
                            <button
                              class={`status-btn disabled-btn ${!prov.enabled ? "active" : ""}`}
                              onClick={() => updateProviderProperty(prov.id, "enabled", false)}
                            >
                              禁用
                            </button>
                          </div>
                        </div>

                        <button class="sp-btn-danger" style="margin-top: 0;" onClick={() => handleDeleteProvider(prov.id)}>
                          删除
                        </button>
                      </div>

                      <div class="sp-form-section" style="margin-top: 0;">
                        <div class="sp-form-row">
                          <span class="sp-form-label">Base URL</span>
                          <input 
                            type="text" 
                            class="sp-input" 
                            value={prov.endpoint}
                            onInput={(e) => updateProviderProperty(prov.id, "endpoint", e.currentTarget.value)}
                          />
                        </div>

                        <div class="sp-form-row">
                          <span class="sp-form-label">API 格式</span>
                            <Select
                              value={prov.apiFormat}
                              onChange={(v) => updateProviderProperty(prov.id, "apiFormat", v as any)}
                              width="100%"
                              options={[
                                { value: "anthropic", label: "Anthropic Messages (/v1/messages)" },
                                { value: "openai", label: "Chat Completions (/chat/completions)" },
                                { value: "responses", label: "Responses (/responses)" },
                              ]}
                            />
                        </div>

                        <div class="sp-form-row">
                          <span class="sp-form-label">API Key</span>
                          <div class="sp-input-wrapper">
                            <input 
                              type={showApiKey() ? "text" : "password"} 
                              class="sp-input password-input" 
                              placeholder={t("apiKeyPlaceholder")}
                              value={prov.apiKey}
                              onInput={(e) => updateProviderProperty(prov.id, "apiKey", e.currentTarget.value)}
                            />
                            <button class="sp-pwd-toggle" onClick={() => setShowApiKey(!showApiKey())}>
                              {showApiKey() ? (
                                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
                                  <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"></path>
                                  <circle cx="12" cy="12" r="3"></circle>
                                </svg>
                              ) : (
                                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
                                  <path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24"></path>
                                  <line x1="1" y1="1" x2="23" y2="23"></line>
                                </svg>
                              )}
                            </button>
                          </div>
                        </div>

                        <div class="sp-form-row" style="margin-top: 10px;">
                          <span class="sp-form-label">模型列表</span>
                          <div class="models-list">
                            <For each={prov.models}>
                              {(model) => (
                                <div class="model-row">
                                  <span class="model-name-lbl">{model.id}</span>
                                  <div style="display: flex; align-items: center; gap: 12px;">
                                    <span class="sp-context-badge" title="上下文窗口">
                                      {model.contextWindow >= 10000 ? `${model.contextWindow / 10000}万` : model.contextWindow}
                                    </span>
                                    <span class="sp-context-badge" title="最大输出 Token">
                                      Out: {model.maxTokens || 4096}
                                    </span>
                                    {/* Per-model test status (in-memory, this session). */}
                                    <ModelStatusIcon
                                      status={modelTests()[prov.id]?.[model.id]?.status}
                                      errorTip={modelTests()[prov.id]?.[model.id]?.msg}
                                    />
                                    <div class="model-actions-btns" style="display: flex; align-items: center; gap: 8px;">

                                      <button class="sp-action-icon-btn" title="编辑模型" onClick={() => handleOpenEditModel(model)}>
                                        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 13px; height: 13px;">
                                          <path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"></path>
                                          <path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"></path>
                                        </svg>
                                      </button>
                                      <button class="sp-action-icon-btn" title="删除模型" onClick={() => handleDeleteModel(model.id)}>
                                        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 13px; height: 13px;">
                                          <polyline points="3 6 5 6 21 6"></polyline>
                                          <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"></path>
                                        </svg>
                                      </button>
                                    </div>
                                  </div>
                                </div>
                              )}
                            </For>
                            <button class="sp-add-model-inline-btn" onClick={handleOpenAddModel}>
                              + 添加模型
                            </button>
                          </div>
                        </div>

                        {/* Batch connection test — tests every model of this provider. */}
                        <div style="display: flex; flex-direction: column; gap: 12px; margin-top: 6px; padding-top: 18px; border-top: 1px solid var(--border-faint);">
                          <div style="display: flex; align-items: center; gap: 12px; flex-wrap: wrap;">
                            <button
                              class="ss-btn ss-btn-secondary btn-sec-no-highlight"
                              style="padding: 6px 16px; font-size: 13px; display: flex; align-items: center; gap: 6px;"
                              onClick={() => handleTestModelConnection(prov)}
                              disabled={batchTesting(prov.id)}
                            >
                              <Show when={batchTesting(prov.id)}>
                                <div class="loader-spinner" style="border-top-color: var(--text-primary); width: 11px; height: 11px; border-width: 1.5px;"></div>
                              </Show>
                              {batchTesting(prov.id) ? t("modelTesting") : t("modelTestBtn")}
                            </button>

                          </div>
                        </div>
                      </div>
                    </>
                  );
                })()}
              </Show>
            </div>
          </div>
        </Show>

        {/* Placeholders for other tabs */}
        <Show when={activeTab() !== "general" && activeTab() !== "modelSettings" && activeTab() !== "commands" && activeTab() !== "codePreview" && activeTab() !== "databases"}>
          <div class="settings-view-header">
            <h2>{t("settings")}</h2>
            <p class="settings-view-subtitle">{t("moduleDeveloping")}...</p>
          </div>
          <div class="settings-panel-box single-col">
            <div class="sp-empty-state">
              <span class="empty-icon">🚧</span>
              <h4>{t("moduleUnderConstruction")}</h4>
              <p>{t("m1WorkingMsg")}</p>
            </div>
          </div>
        </Show>
        </main>
      </div>

      {/* Modal Dialog for Add/Edit Model */}
      <Show when={isModelModalOpen()}>
        <div class="sp-modal-overlay" onClick={() => setIsModelModalOpen(false)}>
          <div class="sp-modal-box" onClick={(e) => e.stopPropagation()}>
            <div class="sp-modal-header">
              <h3>{modalMode() === "add" ? "添加模型" : "编辑模型"}</h3>
              <button class="sp-modal-close" onClick={() => setIsModelModalOpen(false)}>×</button>
            </div>
            
            <div class="sp-modal-body">
              <div class="sp-form-row" style="margin-bottom: 16px;">
                <span class="sp-form-label">模型 ID</span>
                <input 
                  type="text" 
                  class="sp-input" 
                  placeholder="模型 ID"
                  value={modelFormId()}
                  onInput={(e) => setModelFormId(e.currentTarget.value)}
                />
              </div>
              
              <div class="sp-form-row" style="margin-bottom: 16px;">
                <span class="sp-form-label">上下文窗口</span>
                <input 
                  type="number" 
                  class="sp-input" 
                  placeholder="200000"
                  value={modelFormWindow()}
                  onInput={(e) => setModelFormWindow(parseInt(e.currentTarget.value) || 0)}
                />
              </div>

              <div class="sp-form-row">
                <span class="sp-form-label">最大输出 Token</span>
                <input 
                  type="number" 
                  class="sp-input" 
                  placeholder="4096"
                  value={modelFormMaxTokens()}
                  onInput={(e) => setModelFormMaxTokens(parseInt(e.currentTarget.value) || 0)}
                />
              </div>
            </div>
            
            <div class="sp-modal-footer">
              <button class="sp-modal-btn cancel-btn" onClick={() => setIsModelModalOpen(false)}>取消</button>
              <button class="sp-modal-btn save-btn" onClick={handleSaveModel}>保存</button>
            </div>
          </div>
        </div>
      </Show>
    </div>
  );
}
