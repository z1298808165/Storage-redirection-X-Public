/**
 * SRX Core WebUI - KernelSU API Wrapper
 * Abstracts KernelSU WebUI APIs for config management.
 */

const MODULE_DIR = "/data/adb/modules/storage.redirect.x";
const CONFIG_DIR = MODULE_DIR + "/config";
const APPS_DIR = CONFIG_DIR + "/apps";
const TEMPLATES_CONFIG = CONFIG_DIR + "/templates.json";
const FILE_MONITOR_FILTERS_CONFIG = CONFIG_DIR + "/file_monitor_filters.json";
const LOGS_DIR = MODULE_DIR + "/logs";
const MODULE_STATS_FILE = MODULE_DIR + "/stats";
const GLOBAL_CONFIG = CONFIG_DIR + "/global.json";
const RUNTIME_DISABLE = MODULE_DIR + "/.runtime_disabled";
const RUNTIME_STATE_CONFIG = CONFIG_DIR + "/runtime_state.json";
const MODULE_DISABLE = MODULE_DIR + "/disable";
const MODULE_BOOT_OK = MODULE_DIR + "/.boot_ok";
const MODULE_PROP = MODULE_DIR + "/module.prop";
const SRXCTL = MODULE_DIR + "/bin/srxctl";
const DIAGNOSTIC_ARCHIVE_SCRIPT = MODULE_DIR + "/service.d/diagnostic_archive.sh";
const LIST_APPS_DEX = MODULE_DIR + "/bin/list_apps.dex";
const LIST_APPS_OUTPUT = "/data/Namespace-Proxy/list.config";
const FILE_MONITOR_LOG = LOGS_DIR + "/file_monitor.log";
const MODULE_LOG_PACKAGE = "storage.redirect.x";
const DEFAULT_RELEASE_REPOSITORY = "z1298808165/Storage-redirection-X-Public";
const DEFAULT_OFFICIAL_RELEASE_REPOSITORY = "Kindness-Kismet/Storage-redirection-X-Public";
const DEFAULT_RELEASE_BRANCH = "SRX-R";
const UPDATE_MANIFEST_URL =
  "https://raw.githubusercontent.com/" +
  DEFAULT_RELEASE_REPOSITORY +
  "/" +
  DEFAULT_RELEASE_BRANCH +
  "/update.json";
const MOCK_RELEASE_NOTES = `## 模块更新

### 修复了什么问题
- 修复配置重载后文件监视状态不同步的问题。
- 优化 **FUSE**、挂载和路径映射的兼容处理。
- 更新 WebUI 的检查更新弹窗，支持 Markdown 与长内容滚动。

### 注意事项
- 更新模块后建议重启已配置应用和 MediaProvider。
- 涉及运行时挂载的配置需要重新进入应用后生效。

## App 更新

### 增加了什么功能
- 管理 App 的更新弹窗现在会直接显示 Release 更新日志。
- 支持标题、列表、**粗体**、行内代码和 [链接](https://github.com/z1298808165/Storage-redirection-X-Public)。

### 修复了什么问题
- 限制更新日志区域最大高度，按钮始终保持可见。
- 按模块、App 和其它内容分别呈现变更。

## 其它更新

### 构建与文档
- 调整 CI 和 Release 更新日志生成规范。
- 自动从 GitHub Release 回读最终正文并写入更新清单。
- 客户端不会显示提交列表和完整提交对比。
- 这一条用于延长预览内容，方便确认滚动体验。
- 另一条较长的示例内容，用于检查窄屏下文字换行、段落间距和弹窗整体高度是否自然。`;

function hasNativeWebUiBridge() {
  return (
    (typeof window !== "undefined" && typeof window.LSPosedBridge?.exec === "function") ||
    (typeof ksu !== "undefined" && typeof ksu.exec === "function")
  );
}

function mockUpdateManifest() {
  return {
    schema: 1,
    repository: DEFAULT_RELEASE_REPOSITORY,
    stable: {
      version: "9.9.9",
      tag: "v9.9.9-preview",
      title: "Storage Redirect X 本地预览",
      prerelease: false,
      releaseNotes: MOCK_RELEASE_NOTES,
    },
    beta: null,
    releases: [],
  };
}
const MEDIA_PROVIDER_PACKAGES = [
  "com.android.providers.media.module",
  "com.google.android.providers.media.module",
  "com.android.providers.media",
];

// 在 api.js 顶部引入 NativeBridge 适配器
const NativeBridge = {
  async exec(cmd, options) {
    return new Promise((resolve, reject) => {
      // 1. 适配：原生 LSPosed 模块 App 注入的 JavascriptInterface
      if (typeof window !== "undefined" && window.LSPosedBridge && window.LSPosedBridge.exec) {
        const callbackId = "lsp_cb_" + Date.now() + Math.floor(Math.random() * 1000);
        const timeoutMs = Math.max(1000, Number(options?.timeoutMs) || 120000);
        let settled = false;
        const timer = setTimeout(() => {
          if (settled) return;
          settled = true;
          try {
            delete window[callbackId];
          } catch {}
          reject(new Error("Command timed out"));
        }, timeoutMs);
        window[callbackId] = (code, stdout, stderr) => {
          if (settled) return;
          settled = true;
          clearTimeout(timer);
          try {
            delete window[callbackId];
          } catch {}
          if (code === 0) resolve(String(stdout || "").trim());
          else reject(new Error(stderr || "Command failed with code " + code));
        };
        window.LSPosedBridge.exec(cmd, callbackId);
        return;
      }

      // 2. 适配：KernelSU WebUI 环境 (恢复原版完整且稳健的执行与兼容逻辑)
      if (typeof ksu !== "undefined" && ksu.exec) {
        let settled = false;
        let timer = null;
        const callbackName =
          "srx_exec_callback_" + Date.now() + "_" + Math.floor(Math.random() * 100000);
        const cleanup = () => {
          if (timer) clearTimeout(timer);
          try {
            delete window[callbackName];
          } catch {}
        };

        const finish = (code, stdout, stderr) => {
          if (settled) return;
          settled = true;
          cleanup();
          // 处理 KSU 返回对象或拆分参数的差异
          if (typeof code === "object" && code !== null) {
            stdout = code.stdout || code.out || "";
            stderr = code.stderr || code.err || "";
            code = Number(code.code ?? code.exitCode ?? code.errno ?? 0);
          }
          code = Number(code ?? 0);
          if (code === 0) resolve(String(stdout || "").trim());
          else reject(new Error(stderr || "Command failed with code " + code));
        };

        const timeoutMs = Math.max(1000, Number(options?.timeoutMs) || 120000);
        timer = setTimeout(() => finish(124, "", "Command timed out"), timeoutMs);
        window[callbackName] = finish;

        try {
          const result =
            ksu.exec.length === 2 ? ksu.exec(cmd, finish) : ksu.exec(cmd, "{}", callbackName);
          if (result && typeof result.then === "function") {
            result
              .then((res) => {
                if (typeof res === "string") finish(0, res, "");
                else finish(res);
              })
              .catch(reject);
          } else if (typeof result === "string") {
            finish(0, result, "");
          }
        } catch (e) {
          cleanup();
          try {
            const result = ksu.exec(cmd, finish);
            if (result && typeof result.then === "function") {
              result
                .then((res) => (typeof res === "string" ? finish(0, res, "") : finish(res)))
                .catch(reject);
            } else if (typeof result === "string") {
              finish(0, result, "");
            }
          } catch (fallbackError) {
            reject(fallbackError);
          }
        }
        return;
      }

      // 3. Fallback: 浏览器 Mock (恢复调用底部的 Api._mockExec，实现浏览器端动态模拟)
      console.warn("[API] No Native Bridge found, using mock for:", cmd);
      Api._mockExec(cmd).then(resolve).catch(reject);
    });
  },
};

const DEFAULT_GLOBAL_CONFIG = {
  file_monitor_enabled: false,
  fuse_fix_enabled: true,
  fuse_daemon_redirect_enabled: false,
  verbose_logging_enabled: false,
  auto_enable_redirect_for_new_apps: false,
  auto_enable_new_apps_template_id: "",
  app_config_auto_save: false,
};
const DEFAULT_FILE_MONITOR_FILTERS = {
  excluded_paths: ["Android/data"],
  excluded_operations: [
    "open:read",
    "open*:read",
    "provider_open:read",
    "rename*",
    "unlink*",
    "delete*",
    "rmdir*",
    "link*",
    "symlink*",
    "truncate*",
    "ftruncate*",
    "chmod*",
    "fchmod*",
    "utimens*",
    "futimens*",
    "attrib*",
  ],
};

function shellQuote(value) {
  return "'" + String(value).replace(/'/g, "'\\''") + "'";
}

function srxCtlCommand(action) {
  return "/system/bin/sh " + shellQuote(SRXCTL) + " " + action;
}

function withSrxCtlFallback(action, fallback) {
  return (
    "if [ -r " +
    shellQuote(SRXCTL) +
    " ]; then " +
    srxCtlCommand(action) +
    "; else " +
    fallback +
    "; fi"
  );
}

function isManagedTempPath(path) {
  const clean = String(path || "").replace(/\/+$/g, "");
  return (
    clean.length > "/data/local/tmp/srx_".length &&
    clean.startsWith("/data/local/tmp/srx_") &&
    !clean.includes("..")
  );
}

function isManagedWritePath(path) {
  const clean = String(path || "").replace(/\/+$/g, "");
  return !clean.includes("..") && (clean.startsWith(CONFIG_DIR + "/") || isManagedTempPath(clean));
}

function normalizePublicStoragePath(path) {
  const clean = String(path || "")
    .trim()
    .replace(/\\/g, "/")
    .replace(/\/+/g, "/");
  if (!clean || /[\r\n|]/.test(clean)) return "";
  if (!/^\/(?:storage\/emulated\/[0-9]+|sdcard)(?:\/|$)/.test(clean)) return "";
  if (clean.split("/").some((part) => part === "." || part === "..")) return "";
  return clean.replace(/^\/sdcard(?:\/|$)/, "/storage/emulated/0/");
}

function managedTempCleanupCommand(...paths) {
  if (!paths.length || paths.some((path) => !isManagedTempPath(path))) {
    throw new Error("unsafe managed temp path");
  }
  return "rm -rf " + paths.map(shellQuote).join(" ");
}

function publicStorageWriteTestCommand(source, target) {
  const dir = target.substring(0, target.lastIndexOf("/")) || "/storage/emulated/0/Download";
  return (
    "src=" +
    shellQuote(source) +
    "; target=" +
    shellQuote(target) +
    "; dir=" +
    shellQuote(dir) +
    "; " +
    '[ -s "$src" ] || { echo missing_source; exit 0; }; ' +
    'mkdir -p "$dir" 2>/dev/null || true; rm -f "$target" 2>/dev/null || true; ' +
    '(cp "$src" "$target" 2>/dev/null || cat "$src" > "$target" 2>/dev/null) || true; ' +
    'chmod 644 "$target" 2>/dev/null || true; ' +
    'if [ -s "$target" ]; then echo ok; else rm -f "$target" 2>/dev/null || true; echo failed; fi'
  );
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function diagnosticArchiveWorkerScript() {
  return [
    "#!/system/bin/sh",
    "rc=1",
    'if [ -r "$script" ]; then',
    '  /system/bin/sh "$script" "$stage" "$archive" "$progress"',
    "  rc=$?",
    "else",
    '  echo "diagnostic_archive: script missing: $script" >&2',
    "  rc=127",
    "fi",
    'printf "%s\\n" "$rc" > "$done"',
  ].join("\n");
}

function parseDiagnosticProgress(line) {
  const parts = String(line || "")
    .trim()
    .split("|");
  if (parts.length < 3) return null;
  const percent = Math.max(0, Math.min(100, Number.parseInt(parts[0], 10)));
  if (!Number.isFinite(percent)) return null;
  return {
    percent,
    phase: String(parts[1] || "").slice(0, 32),
    message: parts.slice(2).join("|").slice(0, 80) || "正在导出日志",
  };
}

function parseDiagnosticArchivePoll(stdout) {
  let progressLine = "";
  let doneCode = null;
  String(stdout || "")
    .split(/\r?\n/)
    .forEach((line) => {
      if (line.startsWith("__SRX_DONE__=")) {
        const value = line.slice("__SRX_DONE__=".length).trim();
        if (value !== "") doneCode = Number.parseInt(value, 10);
      } else if (line.includes("|")) {
        progressLine = line.trim();
      }
    });
  return {
    progress: parseDiagnosticProgress(progressLine),
    progressLine,
    doneCode: Number.isFinite(doneCode) ? doneCode : null,
  };
}

function diagnosticProgressCommand(progress, percent, phase, message) {
  if (!progress) return "";
  return (
    'printf "%s|%s|%s\\n" ' +
    shellQuote(String(percent)) +
    " " +
    shellQuote(phase) +
    " " +
    shellQuote(message) +
    " > " +
    shellQuote(progress) +
    " 2>/dev/null || true; "
  );
}

function diagnosticArchiveWorkerScript() {
  return [
    "#!/system/bin/sh",
    "rc=1",
    'if [ -r "$script" ]; then',
    '  /system/bin/sh "$script" "$stage" "$archive" "$progress"',
    "  rc=$?",
    "else",
    '  echo "diagnostic_archive: script missing: $script" >&2',
    "  rc=127",
    "fi",
    'printf "%s\\n" "$rc" > "$done"',
  ].join("\n");
}

function parseDiagnosticProgress(line) {
  const parts = String(line || "")
    .trim()
    .split("|");
  if (parts.length < 3) return null;
  const percent = Math.max(0, Math.min(100, Number.parseInt(parts[0], 10)));
  if (!Number.isFinite(percent)) return null;
  return {
    percent,
    phase: String(parts[1] || "").slice(0, 32),
    message: parts.slice(2).join("|").slice(0, 80) || "正在导出日志",
  };
}

function parseDiagnosticArchivePoll(stdout) {
  let progressLine = "";
  let doneCode = null;
  String(stdout || "")
    .split(/\r?\n/)
    .forEach((line) => {
      if (line.startsWith("__SRX_DONE__=")) {
        const value = line.slice("__SRX_DONE__=".length).trim();
        if (value !== "") doneCode = Number.parseInt(value, 10);
      } else if (line.includes("|")) {
        progressLine = line.trim();
      }
    });
  return {
    progress: parseDiagnosticProgress(progressLine),
    progressLine,
    doneCode: Number.isFinite(doneCode) ? doneCode : null,
  };
}

function prepareManagedTempDirCommand(path) {
  if (!isManagedTempPath(path)) throw new Error("unsafe managed temp path");
  return managedTempCleanupCommand(path) + "; mkdir -p " + shellQuote(path);
}

function isSafePackageName(packageName) {
  return /^[A-Za-z0-9_.-]+$/.test(packageName || "");
}

function isSafeUserId(userId) {
  return /^[0-9]+$/.test(String(userId || ""));
}

function parseCompatAppList(content) {
  const apps = [];
  String(content || "")
    .split("\n")
    .forEach((line) => {
      const text = line.trim();
      if (!text || text.startsWith("#")) return;
      const splitAt = text.indexOf("=");
      const packageName = splitAt >= 0 ? text.slice(0, splitAt).trim() : text.split(/\s+/)[0];
      const appLabel = splitAt >= 0 ? text.slice(splitAt + 1).trim() : packageName;
      if (isSafePackageName(packageName))
        apps.push({ packageName, appLabel: appLabel || packageName, isSystem: false });
    });
  return apps;
}

function normalizePackageResult(raw) {
  if (!raw) return [];
  if (Array.isArray(raw)) return raw;
  if (typeof raw === "string") {
    try {
      return JSON.parse(raw || "[]");
    } catch {
      return [];
    }
  }
  return [];
}

function getPackageNameFromInfo(item) {
  if (typeof item === "string") return item;
  if (!item || typeof item !== "object") return "";
  return item.packageName || item.package || item.name || "";
}

function utf8ToBase64(value) {
  const bytes = new TextEncoder().encode(String(value));
  let binary = "";
  const chunkSize = 0x8000;
  for (let i = 0; i < bytes.length; i += chunkSize) {
    binary += String.fromCharCode.apply(null, bytes.subarray(i, i + chunkSize));
  }
  return btoa(binary);
}

function sanitizeFileName(value, fallback) {
  const name = String(value || "")
    .trim()
    .replace(/[\\/:*?"<>|\x00-\x1f]/g, "_")
    .replace(/^\.+/, "")
    .slice(0, 120);
  return name || fallback;
}

function yieldToUi() {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

function hasStorageRootPrefix(path) {
  const lower = String(path || "")
    .replace(/\\/g, "/")
    .replace(/^\/+/, "")
    .toLowerCase();
  return (
    lower === "sdcard" ||
    lower.startsWith("sdcard/") ||
    lower === "storage/emulated" ||
    lower.startsWith("storage/emulated/") ||
    lower === "storage/self/primary" ||
    lower.startsWith("storage/self/primary/") ||
    lower === "data/media" ||
    lower.startsWith("data/media/")
  );
}

function normalizeMonitorFilterPathValue(raw, options) {
  const allowLegacyAbsolute = options?.allowLegacyAbsolute !== false;
  let value = String(raw || "")
    .trim()
    .replace(/\\/g, "/")
    .replace(/\/+/g, "/");
  if (!value || value.includes("\0") || value.length > 512 || value.startsWith("!")) return "";
  if (hasStorageRootPrefix(value)) return "";
  if (value.startsWith("/")) {
    if (!allowLegacyAbsolute) return "";
    value = value.replace(/^\/+/, "");
  }
  value = value.replace(/^\/+|\/+$/g, "");
  if (!value || value.split("/").some((part) => part === "." || part === "..")) return "";
  if (hasStorageRootPrefix(value) || /[<>:"|\x00-\x1f]/.test(value)) return "";
  return value;
}

function normalizeMonitorFilterPathList(list) {
  const items = Array.isArray(list) ? list : typeof list === "string" ? [list] : [];
  const seen = new Set();
  const out = [];
  items.forEach((item) => {
    const value = normalizeMonitorFilterPathValue(item, { allowLegacyAbsolute: true });
    if (!value || seen.has(value)) return;
    seen.add(value);
    out.push(value);
  });
  return out.slice(0, 200);
}

function normalizeMonitorFilterOperationList(list) {
  const items = Array.isArray(list) ? list : typeof list === "string" ? [list] : [];
  const seen = new Set();
  const out = [];
  items.forEach((item) => {
    const value = String(item || "").trim();
    if (!value || value.includes("\0") || value.length > 512 || seen.has(value)) return;
    seen.add(value);
    out.push(value);
  });
  const normalized = out.slice(0, 200);
  return isLegacyDefaultMonitorOperations(normalized)
    ? DEFAULT_FILE_MONITOR_FILTERS.excluded_operations.slice()
    : normalized;
}

function isLegacyDefaultMonitorOperations(list) {
  if (!Array.isArray(list)) return false;
  const normalized = list
    .map((item) => String(item || "").toLowerCase())
    .sort()
    .join("\n");
  return (
    normalized === ["delete*", "open:read", "rename*", "unlink*"].join("\n") ||
    normalized ===
      [
        "attrib*",
        "chmod*",
        "delete*",
        "fchmod*",
        "ftruncate*",
        "futimens*",
        "link*",
        "open*:read",
        "open:read",
        "rename*",
        "rmdir*",
        "symlink*",
        "truncate*",
        "unlink*",
        "utimens*",
      ].join("\n")
  );
}

function normalizeMonitorFilters(config) {
  return {
    excluded_paths: normalizeMonitorFilterPathList(config?.excluded_paths),
    excluded_operations: normalizeMonitorFilterOperationList(config?.excluded_operations),
  };
}

function parseSemVersion(value) {
  const match = String(value || "")
    .trim()
    .match(/(?:^|[^0-9])v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?/);
  if (!match) return null;
  return {
    major: Number.parseInt(match[1], 10),
    minor: Number.parseInt(match[2], 10),
    patch: Number.parseInt(match[3], 10),
    preRelease: match[4] ? match[4].split(".").filter(Boolean) : [],
  };
}

function comparePreRelease(left, right) {
  const max = Math.max(left.length, right.length);
  for (let i = 0; i < max; i++) {
    if (left[i] == null) return -1;
    if (right[i] == null) return 1;
    const leftNumber = /^\d+$/.test(left[i]) ? Number.parseInt(left[i], 10) : null;
    const rightNumber = /^\d+$/.test(right[i]) ? Number.parseInt(right[i], 10) : null;
    if (leftNumber != null && rightNumber != null) {
      if (leftNumber !== rightNumber) return leftNumber - rightNumber;
    } else if (leftNumber != null) {
      return -1;
    } else if (rightNumber != null) {
      return 1;
    } else if (left[i] !== right[i]) {
      return left[i] < right[i] ? -1 : 1;
    }
  }
  return 0;
}

function compareSemVersion(left, right) {
  for (const key of ["major", "minor", "patch"]) {
    if (left[key] !== right[key]) return left[key] - right[key];
  }
  if (!left.preRelease.length && right.preRelease.length) return 1;
  if (left.preRelease.length && !right.preRelease.length) return -1;
  return comparePreRelease(left.preRelease, right.preRelease);
}

function normalizeUpdateChannel(channel) {
  return ["Stable", "Beta", "All"].includes(channel) ? channel : "Stable";
}

function manifestReleaseEntries(manifest) {
  const entries = [];
  if (manifest?.stable && hasReleaseVersion(manifest.stable))
    entries.push({ channel: "Stable", release: manifest.stable });
  if (manifest?.beta && hasReleaseVersion(manifest.beta))
    entries.push({ channel: "Beta", release: Object.assign({ prerelease: true }, manifest.beta) });
  (Array.isArray(manifest?.releases) ? manifest.releases : []).forEach((release) => {
    if (!hasReleaseVersion(release)) return;
    const version = parseSemVersion(release.version || release.tag);
    entries.push({
      channel: release.prerelease || version?.preRelease?.length ? "Beta" : "Stable",
      release,
    });
  });
  return entries;
}

function hasReleaseVersion(release) {
  return !!String(release?.version || release?.tag || "").trim();
}

function releaseMatchesChannel(entry, channel) {
  if (channel === "Stable") return entry.channel === "Stable";
  if (channel === "Beta") return entry.channel === "Beta";
  return true;
}

function findReleaseUpdate(manifest, repository, currentVersionName, channel) {
  const selectedChannel = normalizeUpdateChannel(channel);
  const currentVersion = parseSemVersion(currentVersionName) || {
    major: 0,
    minor: 0,
    patch: 0,
    preRelease: [],
  };
  let best = null;
  manifestReleaseEntries(manifest)
    .filter((entry) => releaseMatchesChannel(entry, selectedChannel))
    .forEach((entry) => {
      const release = entry.release;
      const version = parseSemVersion(release.version || release.tag);
      if (!version || compareSemVersion(version, currentVersion) <= 0) return;
      if (!best || compareSemVersion(version, best.version) > 0) best = { entry, version };
    });
  if (!best) return null;
  const release = best.entry.release;
  const releaseRepository = String(
    release.repository || manifest?.repository || repository || DEFAULT_RELEASE_REPOSITORY,
  ).trim();
  const tag = String(release.tag || release.version || "").trim();
  return {
    tagName: tag,
    versionName: String(release.version || tag).trim(),
    title: String(release.title || tag).trim(),
    htmlUrl:
      String(release.url || "").trim() ||
      "https://github.com/" + releaseRepository + "/releases/tag/" + tag,
    channel: best.entry.channel,
    prerelease: best.entry.channel === "Beta" || release.prerelease === true,
    downloadUrl: String(release.downloadUrl || "").trim(),
    moduleUrl: String(release.moduleUrl || "").trim(),
    releaseNotes: String(release.releaseNotes || "").trim(),
  };
}

const Api = {
  _packageInfo: new Map(),
  _compatAppListCache: new Map(),
  _compatAppListPrepared: new Set(),
  _configuredAppsCache: null,
  _configuredAppsCacheAt: 0,
  _globalConfigCache: null,
  _monitorFiltersCache: null,
  _templatesCache: null,

  /**
   * Execute a shell command as root via KernelSU.
   * Falls back to mock data in non-KernelSU environments.
   */
  async exec(cmd, options) {
    return await NativeBridge.exec(cmd, options);
  },

  /** Read Android 12+ wallpaper-derived accent resources without modifying system settings. */
  async readSystemAccentPalette() {
    const names = [
      "system_accent1_600",
      "system_accent1_200",
      "system_accent2_600",
      "system_accent2_200",
    ];
    const command =
      "for name in " +
      names.join(" ") +
      '; do value=$(/system/bin/cmd overlay lookup android android:color/$name 2>/dev/null | tail -n 1); printf "%s=%s\\n" "$name" "$value"; done';
    try {
      const output = await this.exec(command, { timeoutMs: 5000 });
      const colors = {};
      String(output || "")
        .split(/\r?\n/)
        .forEach((line) => {
          const separator = line.indexOf("=");
          if (separator <= 0) return;
          const name = line.slice(0, separator).trim();
          const match = line
            .slice(separator + 1)
            .trim()
            .match(/(?:#|0x)([0-9a-f]{8}|[0-9a-f]{6})\b/i);
          if (!names.includes(name) || !match) return;
          const value = match[1].length === 8 ? match[1].slice(2) : match[1];
          colors[name] = "#" + value.toUpperCase();
        });
      const primary = colors.system_accent1_600 || colors.system_accent1_200;
      if (!primary) return null;
      return {
        lightPrimary: colors.system_accent1_600 || primary,
        darkPrimary: colors.system_accent1_200 || primary,
        lightSecondary: colors.system_accent2_600 || primary,
        darkSecondary: colors.system_accent2_200 || primary,
      };
    } catch {
      return null;
    }
  },

  invalidateConfiguredAppsCache() {
    this._configuredAppsCache = null;
    this._configuredAppsCacheAt = 0;
  },

  invalidateGlobalConfigCache() {
    this._globalConfigCache = null;
  },

  invalidateMonitorFiltersCache() {
    this._monitorFiltersCache = null;
  },

  invalidateTemplatesCache() {
    this._templatesCache = null;
  },

  /** Read file content as string */
  async readFile(path) {
    try {
      return await this.exec("cat " + shellQuote(path) + " 2>/dev/null");
    } catch {
      return "";
    }
  },

  /** Read only the tail of a file to keep dashboard refreshes cheap. */
  async readFileTail(path, lineCount) {
    const lines = Math.max(1, Math.min(5000, Number(lineCount) || 500));
    try {
      const out = await this.exec("tail -n " + lines + " " + shellQuote(path) + " 2>/dev/null");
      return out || (await this.readFile(path));
    } catch {
      return await this.readFile(path);
    }
  },

  /** Write content to file */
  async writeFile(path, content) {
    if (!isManagedWritePath(path)) return false;
    const tmpfile = "/data/local/tmp/srx_tmp_" + Date.now() + ".json";
    try {
      const targetDir = path.substring(0, path.lastIndexOf("/")) || CONFIG_DIR;
      const encoded = utf8ToBase64(content);
      await this.exec(
        "mkdir -p " +
          shellQuote(targetDir) +
          " " +
          shellQuote(APPS_DIR) +
          " && " +
          "printf %s " +
          shellQuote(encoded) +
          " | base64 -d > " +
          shellQuote(tmpfile) +
          " && " +
          "cp " +
          shellQuote(tmpfile) +
          " " +
          shellQuote(path) +
          " && " +
          "chmod 644 " +
          shellQuote(path) +
          " && " +
          "rm -f " +
          shellQuote(tmpfile) +
          " && " +
          "touch " +
          shellQuote(APPS_DIR) +
          " 2>/dev/null && touch " +
          shellQuote(path) +
          " 2>/dev/null",
      );
      this.invalidateConfiguredAppsCache();
      return true;
    } catch (e) {
      console.error("[api] writeFile failed:", e);
      return false;
    }
  },

  /** Write content to an arbitrary file without touching the live config marker. */
  async writeRawFile(path, content, options) {
    const token = Date.now() + "_" + Math.floor(Math.random() * 100000);
    const tmpfile = "/data/local/tmp/srx_raw_" + token;
    const tmpb64 = tmpfile + ".b64";
    const targetDir = path.substring(0, path.lastIndexOf("/")) || "/data/local/tmp";
    const mode = options?.mode || "644";
    const encoded = utf8ToBase64(content);
    const chunkSize = 60000;
    try {
      await this.exec("mkdir -p " + shellQuote(targetDir));
      if (encoded.length <= chunkSize) {
        await this.exec(
          "printf %s " + shellQuote(encoded) + " | base64 -d > " + shellQuote(tmpfile),
        );
      } else {
        await this.exec(": > " + shellQuote(tmpb64));
        for (let i = 0; i < encoded.length; i += chunkSize) {
          await this.exec(
            "printf %s " +
              shellQuote(encoded.slice(i, i + chunkSize)) +
              " >> " +
              shellQuote(tmpb64),
          );
        }
        await this.exec("base64 -d " + shellQuote(tmpb64) + " > " + shellQuote(tmpfile));
      }
      await this.exec(
        "cp " +
          shellQuote(tmpfile) +
          " " +
          shellQuote(path) +
          " && " +
          "chmod " +
          shellQuote(mode) +
          " " +
          shellQuote(path) +
          " && " +
          "rm -f " +
          shellQuote(tmpfile) +
          " " +
          shellQuote(tmpb64),
      );
      return true;
    } catch (e) {
      try {
        await this.exec("rm -f " + shellQuote(tmpfile) + " " + shellQuote(tmpb64));
      } catch {}
      console.error("[api] writeRawFile failed:", e);
      return false;
    }
  },

  /** Save a generated backup to public Downloads when the WebView has no save picker. */
  async saveBackupToDownloads(fileName, content) {
    const safeName = sanitizeFileName(fileName, "srx-backup.srxbak.json");
    const candidates = ["/storage/emulated/0/Download/" + safeName, "/sdcard/Download/" + safeName];
    for (const path of candidates) {
      const ok = await this.writeRawFile(path, content, { mode: "644" });
      if (ok) {
        await this.recordModuleExportMonitor(path, "backup");
        return path;
      }
    }
    return "";
  },

  async writeRawBase64File(path, encoded, options) {
    const token = Date.now() + "_" + Math.floor(Math.random() * 100000);
    const tmpfile = "/data/local/tmp/srx_raw_" + token;
    const tmpb64 = tmpfile + ".b64";
    const targetDir = path.substring(0, path.lastIndexOf("/")) || "/data/local/tmp";
    const mode = options?.mode || "644";
    const value = String(encoded || "");
    const chunkSize = 60000;
    try {
      await this.exec("mkdir -p " + shellQuote(targetDir));
      if (value.length <= chunkSize) {
        await this.exec("printf %s " + shellQuote(value) + " | base64 -d > " + shellQuote(tmpfile));
      } else {
        await this.exec(": > " + shellQuote(tmpb64));
        for (let i = 0; i < value.length; i += chunkSize) {
          await this.exec(
            "printf %s " + shellQuote(value.slice(i, i + chunkSize)) + " >> " + shellQuote(tmpb64),
          );
        }
        await this.exec("base64 -d " + shellQuote(tmpb64) + " > " + shellQuote(tmpfile));
      }
      await this.exec(
        "cp " +
          shellQuote(tmpfile) +
          " " +
          shellQuote(path) +
          " && " +
          "chmod " +
          shellQuote(mode) +
          " " +
          shellQuote(path) +
          " && " +
          "rm -f " +
          shellQuote(tmpfile) +
          " " +
          shellQuote(tmpb64),
      );
      return true;
    } catch (e) {
      try {
        await this.exec("rm -f " + shellQuote(tmpfile) + " " + shellQuote(tmpb64));
      } catch {}
      console.error("[api] writeRawBase64File failed:", e);
      return false;
    }
  },

  async saveBackupBytesToDownloads(fileName, encoded) {
    const safeName = sanitizeFileName(fileName, "srx-backup.srxbak.zip");
    const candidates = ["/storage/emulated/0/Download/" + safeName, "/sdcard/Download/" + safeName];
    for (const path of candidates) {
      const ok = await this.writeRawBase64File(path, encoded, { mode: "644" });
      if (ok) {
        await this.recordModuleExportMonitor(path, "backup");
        return path;
      }
    }
    return "";
  },

  async recordModuleExportMonitor(path, kind) {
    const target = normalizePublicStoragePath(path);
    const exportKind = kind === "backup" ? "backup" : "diagnostic";
    const source = exportKind === "backup" ? "webui_backup" : "webui_export";
    if (!target) return false;
    try {
      await this.exec(
        "mkdir -p " +
          shellQuote(LOGS_DIR) +
          " && " +
          'ts=$(date "+%Y-%m-%d %H:%M:%S" 2>/dev/null || toybox date "+%Y-%m-%d %H:%M:%S" 2>/dev/null); ' +
          'printf "%s|' +
          MODULE_LOG_PACKAGE +
          "|" +
          MODULE_LOG_PACKAGE +
          '|OPEN|%s|ret=0|errno=0|identify_method=module_export|identify_reliability=high|op=provider_open|op_filter=provider_open:write|source=%s|export_kind=%s\\n" ' +
          '"${ts:-unknown}" ' +
          shellQuote(target) +
          " " +
          shellQuote(source) +
          " " +
          shellQuote(exportKind) +
          " >> " +
          shellQuote(FILE_MONITOR_LOG) +
          " && " +
          "chmod 666 " +
          shellQuote(FILE_MONITOR_LOG) +
          " 2>/dev/null || true",
      );
      return true;
    } catch (e) {
      console.warn("[api] recordModuleExportMonitor failed:", e);
      return false;
    }
  },

  buildDiagnosticArchiveCommand(stage, archive, progress) {
    if (
      !isManagedTempPath(stage) ||
      !isManagedTempPath(archive) ||
      (progress && !isManagedTempPath(progress))
    )
      throw new Error("unsafe diagnostic temp path");
    const progressArg = progress ? " " + shellQuote(progress) : "";
    const scriptCommand =
      "if [ -r " +
      shellQuote(DIAGNOSTIC_ARCHIVE_SCRIPT) +
      " ]; then " +
      "/system/bin/sh " +
      shellQuote(DIAGNOSTIC_ARCHIVE_SCRIPT) +
      " " +
      shellQuote(stage) +
      " " +
      shellQuote(archive) +
      progressArg +
      "; " +
      "rc=$?; [ $rc -eq 0 ] && exit 0; fi; ";
    return scriptCommand + this.buildLegacyDiagnosticArchiveCommand(stage, archive, progress);
  },

  buildDiagnosticArchiveStartCommand(stage, archive, progress, done, runLog, pid) {
    const worker = progress + ".worker.sh";
    const paths = [stage, archive, progress, progress + ".tmp", done, runLog, pid, worker];
    if (paths.some((path) => !isManagedTempPath(path)))
      throw new Error("unsafe diagnostic temp path");
    return (
      managedTempCleanupCommand(...paths) +
      "; " +
      "stage=" +
      shellQuote(stage) +
      "; archive=" +
      shellQuote(archive) +
      "; progress=" +
      shellQuote(progress) +
      "; " +
      "done=" +
      shellQuote(done) +
      "; run_log=" +
      shellQuote(runLog) +
      "; pid_file=" +
      shellQuote(pid) +
      "; " +
      "worker=" +
      shellQuote(worker) +
      "; script=" +
      shellQuote(DIAGNOSTIC_ARCHIVE_SCRIPT) +
      "; " +
      'printf "%s|%s|%s\\n" "1" "start" "正在启动日志导出" > "$progress" 2>/dev/null || true; ' +
      "cat > \"$worker\" <<'SRX_DIAG_WORKER'\n" +
      diagnosticArchiveWorkerScript() +
      "\nSRX_DIAG_WORKER\n" +
      'chmod 700 "$worker" 2>/dev/null || true; ' +
      "export stage archive progress done script; " +
      "if command -v setsid >/dev/null 2>&1; then " +
      'setsid /system/bin/sh "$worker" > "$run_log" 2>&1 < /dev/null & worker_pid=$!; ' +
      'else /system/bin/sh "$worker" > "$run_log" 2>&1 < /dev/null & worker_pid=$!; fi; ' +
      'printf "%s\\n" "$worker_pid" > "$pid_file"; exit 0'
    );
  },

  buildDiagnosticArchivePollCommand(progress, done) {
    if (!isManagedTempPath(progress) || !isManagedTempPath(done))
      throw new Error("unsafe diagnostic temp path");
    return (
      "progress=" +
      shellQuote(progress) +
      "; done=" +
      shellQuote(done) +
      "; " +
      'if [ -f "$progress" ]; then tail -n 1 "$progress"; fi; ' +
      'printf "\\n__SRX_DONE__="; if [ -f "$done" ]; then cat "$done"; fi'
    );
  },

  buildLegacyDiagnosticArchiveCommand(stage, archive, progress) {
    return (
      "stage=" +
      shellQuote(stage) +
      "; archive=" +
      shellQuote(archive) +
      "; module=" +
      shellQuote(MODULE_DIR) +
      "; logs=" +
      shellQuote(LOGS_DIR) +
      "; config=" +
      shellQuote(CONFIG_DIR) +
      "; " +
      diagnosticProgressCommand(progress, 5, "legacy", "正在使用兼容模式导出日志") +
      'rm -rf "$stage" "$archive"; mkdir -p "$stage/logs" "$stage/config" "$stage/state" || exit 1; ' +
      diagnosticProgressCommand(progress, 18, "files", "正在复制模块日志和配置") +
      'if [ -d "$logs" ]; then find "$logs" -maxdepth 1 -type f ! -name ".*.pid" ! -name ".uid_map_last_refresh" -exec cp -p {} "$stage/logs/" \\; 2>/dev/null; fi; ' +
      'cp -p "$module/module.prop" "$stage/module.prop" 2>/dev/null || true; ' +
      'cp -p "$module/stats" "$stage/stats" 2>/dev/null || true; ' +
      'cp -p "$config/global.json" "$stage/config/global.json" 2>/dev/null || true; ' +
      'cp -p "$config/file_monitor_filters.json" "$stage/config/file_monitor_filters.json" 2>/dev/null || true; ' +
      'cp -p "$config/templates.json" "$stage/config/templates.json" 2>/dev/null || true; ' +
      diagnosticProgressCommand(progress, 45, "state", "正在采集基础状态") +
      '{ date; id; uname -a; getprop ro.build.fingerprint 2>/dev/null; getprop ro.product.model 2>/dev/null; getprop ro.build.version.release 2>/dev/null; } > "$stage/state/device.txt" 2>&1; ' +
      '{ /system/bin/sh "$module/bin/srxctl" status 2>/dev/null || true; ls -la "$module" 2>/dev/null; ls -la "$logs" 2>/dev/null; } > "$stage/state/module.txt" 2>&1; ' +
      '{ ps -A 2>/dev/null | grep -E "srx|zygisk|media|storage" || true; } > "$stage/state/processes.txt" 2>&1; ' +
      '{ for p in com.android.providers.media.module com.google.android.providers.media.module com.android.providers.media android.process.media; do echo "## pidof $p"; pidof "$p" 2>/dev/null || true; done; } > "$stage/state/media-pids.txt" 2>&1; ' +
      diagnosticProgressCommand(progress, 72, "logcat", "正在截取系统日志") +
      'logcat -d -t 2000 -v threadtime -s StorageRedirect:V SRX:V FileMonitorOp:I Stats:I AndroidRuntime:E DEBUG:F libc:F > "$stage/logcat-threadtime.txt" 2>&1 || true; ' +
      'dmesg 2>/dev/null | tail -n 1000 > "$stage/dmesg-tail.txt" 2>/dev/null || true; ' +
      diagnosticProgressCommand(progress, 95, "archive", "正在压缩日志包") +
      '(cd "$stage" && tar -czf "$archive" *) || exit 1; chmod 644 "$archive"; rm -rf "$stage"'
    );
  },

  buildDiagnosticArchiveStartCommand(stage, archive, progress, done, runLog, pid) {
    const worker = progress + ".worker.sh";
    const paths = [stage, archive, progress, progress + ".tmp", done, runLog, pid, worker];
    if (paths.some((path) => !isManagedTempPath(path)))
      throw new Error("unsafe diagnostic temp path");
    return (
      managedTempCleanupCommand(...paths) +
      "; " +
      "stage=" +
      shellQuote(stage) +
      "; archive=" +
      shellQuote(archive) +
      "; progress=" +
      shellQuote(progress) +
      "; " +
      "done=" +
      shellQuote(done) +
      "; run_log=" +
      shellQuote(runLog) +
      "; pid_file=" +
      shellQuote(pid) +
      "; " +
      "worker=" +
      shellQuote(worker) +
      "; script=" +
      shellQuote(DIAGNOSTIC_ARCHIVE_SCRIPT) +
      "; " +
      'printf "%s|%s|%s\\n" "1" "start" "正在启动日志导出" > "$progress" 2>/dev/null || true; ' +
      "cat > \"$worker\" <<'SRX_DIAG_WORKER'\n" +
      diagnosticArchiveWorkerScript() +
      "\nSRX_DIAG_WORKER\n" +
      'chmod 700 "$worker" 2>/dev/null || true; ' +
      "export stage archive progress done script; " +
      "if command -v setsid >/dev/null 2>&1; then " +
      'setsid /system/bin/sh "$worker" > "$run_log" 2>&1 < /dev/null & worker_pid=$!; ' +
      'else /system/bin/sh "$worker" > "$run_log" 2>&1 < /dev/null & worker_pid=$!; fi; ' +
      'printf "%s\\n" "$worker_pid" > "$pid_file"; exit 0'
    );
  },

  buildDiagnosticArchivePollCommand(progress, done) {
    if (!isManagedTempPath(progress) || !isManagedTempPath(done))
      throw new Error("unsafe diagnostic temp path");
    return (
      "progress=" +
      shellQuote(progress) +
      "; done=" +
      shellQuote(done) +
      "; " +
      'if [ -f "$progress" ]; then tail -n 1 "$progress"; fi; ' +
      'printf "\\n__SRX_DONE__="; if [ -f "$done" ]; then cat "$done"; fi'
    );
  },

  async runDiagnosticArchive(stage, archive, progress, done, runLog, pid, options) {
    // 后台执行 + 轮询进度文件
    options?.onProgress?.({ percent: 1, phase: "start", message: "正在启动日志导出" });

    const script = DIAGNOSTIC_ARCHIVE_SCRIPT;
    const startCommand =
      "stage=" +
      shellQuote(stage) +
      "; " +
      "archive=" +
      shellQuote(archive) +
      "; " +
      "progress=" +
      shellQuote(progress) +
      "; " +
      "done=" +
      shellQuote(done) +
      "; " +
      "script=" +
      shellQuote(script) +
      "; " +
      managedTempCleanupCommand(stage, archive, progress, progress + ".tmp", done) +
      "; " +
      'if [ ! -r "$script" ]; then echo "script not found: $script" >&2; exit 127; fi; ' +
      "(" +
      '  /system/bin/sh "$script" "$stage" "$archive" "$progress"; ' +
      '  echo $? > "$done"; ' +
      ") >/dev/null 2>&1 &";

    // 启动后台任务
    await this.exec(startCommand, { timeoutMs: 5000 });

    // 轮询进度
    const deadline = Date.now() + 180000;
    let lastPercent = 1;
    while (Date.now() < deadline) {
      await sleep(800);

      // 检查是否完成
      const doneCode = await this.exec(
        "[ -f " + shellQuote(done) + " ] && cat " + shellQuote(done) + ' || echo ""',
        { timeoutMs: 3000 },
      ).catch(() => "");
      if (doneCode.trim() !== "") {
        const code = parseInt(doneCode.trim());
        if (code !== 0) throw new Error("日志打包失败 (exit " + code + ")");
        break;
      }

      // 读取进度
      const progressText = await this.exec(
        "[ -f " + shellQuote(progress) + " ] && tail -n 1 " + shellQuote(progress) + ' || echo ""',
        { timeoutMs: 3000 },
      ).catch(() => "");
      const match = progressText.match(/^(\d+)\|([^|]*)\|(.*)$/);
      if (match) {
        const phase = match[2] || "running";
        const message = match[3] || "正在导出日志";
        let percent = parseInt(match[1]) || lastPercent;
        if (percent >= 100 && phase === "done") {
          percent = 98;
        }
        if (percent > lastPercent) {
          lastPercent = percent;
          options?.onProgress?.({
            percent,
            phase,
            message:
              percent >= 98 && phase === "done" ? "日志包已生成，正在准备写入目标文件" : message,
          });
        }
      }
    }

    options?.onProgress?.({ percent: 99, phase: "verify", message: "正在确认日志包" });
    const exists = await this.exec("[ -s " + shellQuote(archive) + " ] && echo 1 || echo 0", {
      timeoutMs: 10000,
    }).catch(() => "0");
    if (String(exists).trim() !== "1") throw new Error("日志包生成失败");
    return true;
  },

  async copyDiagnosticArchiveToPublicPath(source, target) {
    if (!isManagedTempPath(source)) return false;
    const writeTarget = String(target || "")
      .trim()
      .replace(/\\/g, "/")
      .replace(/\/+/g, "/");
    if (!normalizePublicStoragePath(writeTarget)) return false;
    const result = await this.exec(publicStorageWriteTestCommand(source, writeTarget), {
      timeoutMs: 60000,
    }).catch(() => "");
    return (
      String(result || "")
        .trim()
        .split(/\s+/)
        .pop() === "ok"
    );
  },

  async exportDiagnosticArchiveToDownloads(fileName, options) {
    const safeName = sanitizeFileName(fileName, "storage-redirect-x-logs.tar.gz");
    const candidates = ["/storage/emulated/0/Download/" + safeName, "/sdcard/Download/" + safeName];
    const token = Date.now() + "_" + Math.floor(Math.random() * 100000);
    const stage = "/data/local/tmp/srx_diag_" + token;
    const tmpArchive = "/data/local/tmp/srx_diag_archive_" + token + ".tar.gz";
    const progress = "/data/local/tmp/srx_diag_progress_" + token;
    const done = progress + ".done";
    const runLog = progress + ".log";
    const pid = progress + ".pid";
    try {
      await this.runDiagnosticArchive(stage, tmpArchive, progress, done, runLog, pid, options);
      options?.onProgress?.({ percent: 99, phase: "copy", message: "正在写入目标文件" });
      for (const target of candidates) {
        if (await this.copyDiagnosticArchiveToPublicPath(tmpArchive, target)) {
          options?.onProgress?.({ percent: 100, phase: "done", message: "日志包已保存" });
          await this.recordModuleExportMonitor(target, "diagnostic");
          return target;
        }
      }
      throw new Error("日志包写入 Download 失败");
    } finally {
      try {
        await this.exec(
          managedTempCleanupCommand(
            stage,
            tmpArchive,
            progress,
            progress + ".tmp",
            done,
            runLog,
            pid,
            progress + ".worker.sh",
          ),
        );
      } catch {}
    }
  },

  async exportDiagnosticArchiveToDirectory(dirPath, fileName, options) {
    const safeName = sanitizeFileName(fileName, "storage-redirect-x-logs.tar.gz");
    const dir = String(dirPath || "")
      .trim()
      .replace(/\\/g, "/")
      .replace(/\/+/g, "/")
      .replace(/\/+$/g, "");
    if (!dir || !/^\/(?:storage\/emulated\/[0-9]+|sdcard)(?:\/|$)/.test(dir)) return "";
    if (dir.split("/").some((part) => part === "." || part === "..")) return "";
    const target = dir + "/" + safeName;
    const token = Date.now() + "_" + Math.floor(Math.random() * 100000);
    const stage = "/data/local/tmp/srx_diag_" + token;
    const tmpArchive = "/data/local/tmp/srx_diag_archive_" + token + ".tar.gz";
    const progress = "/data/local/tmp/srx_diag_progress_" + token;
    const done = progress + ".done";
    const runLog = progress + ".log";
    const pid = progress + ".pid";
    try {
      await this.runDiagnosticArchive(stage, tmpArchive, progress, done, runLog, pid, options);
      options?.onProgress?.({ percent: 99, phase: "copy", message: "正在写入目标文件" });
      if (!(await this.copyDiagnosticArchiveToPublicPath(tmpArchive, target))) {
        throw new Error("日志包写入所选目录失败");
      }
      options?.onProgress?.({ percent: 100, phase: "done", message: "日志包已保存" });
      await this.recordModuleExportMonitor(target, "diagnostic");
      return target;
    } finally {
      try {
        await this.exec(
          managedTempCleanupCommand(
            stage,
            tmpArchive,
            progress,
            progress + ".tmp",
            done,
            runLog,
            pid,
            progress + ".worker.sh",
          ),
        );
      } catch {}
    }
  },

  /** Save a generated backup to a user-selected public storage directory. */
  async saveBackupToDirectory(dirPath, fileName, content) {
    const safeName = sanitizeFileName(fileName, "srx-backup.srxbak.json");
    const dir = String(dirPath || "")
      .trim()
      .replace(/\\/g, "/")
      .replace(/\/+/g, "/")
      .replace(/\/+$/g, "");
    if (!dir || !/^\/(?:storage\/emulated\/[0-9]+|sdcard)(?:\/|$)/.test(dir)) return "";
    if (dir.split("/").some((part) => part === "." || part === "..")) return "";
    const target = dir + "/" + safeName;
    const ok = await this.writeRawFile(target, content, { mode: "644" });
    if (ok) await this.recordModuleExportMonitor(target, "backup");
    return ok ? target : "";
  },

  async saveBackupBytesToDirectory(dirPath, fileName, encoded) {
    const safeName = sanitizeFileName(fileName, "srx-backup.srxbak.zip");
    const dir = String(dirPath || "")
      .trim()
      .replace(/\\/g, "/")
      .replace(/\/+/g, "/")
      .replace(/\/+$/g, "");
    if (!dir || !/^\/(?:storage\/emulated\/[0-9]+|sdcard)(?:\/|$)/.test(dir)) return "";
    if (dir.split("/").some((part) => part === "." || part === "..")) return "";
    const target = dir + "/" + safeName;
    const ok = await this.writeRawBase64File(target, encoded, { mode: "644" });
    if (ok) await this.recordModuleExportMonitor(target, "backup");
    return ok ? target : "";
  },

  /** Check if file exists */
  async fileExists(path) {
    try {
      const out = await this.exec("test -f " + shellQuote(path) + " && echo 1 || echo 0");
      return out === "1";
    } catch {
      return false;
    }
  },

  /** Read global config as parsed object */
  async readGlobalConfig(options) {
    if (options?.force) this.invalidateGlobalConfigCache();
    if (this._globalConfigCache) return Object.assign({}, this._globalConfigCache);
    const content = await this.readFile(GLOBAL_CONFIG);
    if (!content) {
      this._globalConfigCache = Object.assign({}, DEFAULT_GLOBAL_CONFIG);
      return Object.assign({}, this._globalConfigCache);
    }
    try {
      this._globalConfigCache = Object.assign({}, DEFAULT_GLOBAL_CONFIG, JSON.parse(content));
      return Object.assign({}, this._globalConfigCache);
    } catch {
      this._globalConfigCache = Object.assign({}, DEFAULT_GLOBAL_CONFIG);
      return Object.assign({}, this._globalConfigCache);
    }
  },

  /** Read the cumulative success counter used by the dashboard. */
  async readStatsCount() {
    try {
      const out = await this.readFile(MODULE_STATS_FILE);
      const count = Number.parseInt(String(out || "").trim(), 10);
      return Number.isFinite(count) && count >= 0 ? count : null;
    } catch {
      return null;
    }
  },

  /** Write global config */
  async writeGlobalConfig(config) {
    const json = JSON.stringify(config, null, 2);
    const ok = await this.writeFile(GLOBAL_CONFIG, json);
    if (ok) {
      this._globalConfigCache = Object.assign({}, config);
      if (this._mockStore) this._mockStore.global = Object.assign({}, config);
    }
    return ok;
  },

  async readMonitorFilters(options) {
    if (options?.force) this.invalidateMonitorFiltersCache();
    if (this._monitorFiltersCache) return JSON.parse(JSON.stringify(this._monitorFiltersCache));
    const content = await this.readFile(FILE_MONITOR_FILTERS_CONFIG);
    if (!content) {
      this._monitorFiltersCache = JSON.parse(JSON.stringify(DEFAULT_FILE_MONITOR_FILTERS));
      return JSON.parse(JSON.stringify(this._monitorFiltersCache));
    }
    try {
      const parsed = JSON.parse(content);
      this._monitorFiltersCache = normalizeMonitorFilters(parsed);
      return JSON.parse(JSON.stringify(this._monitorFiltersCache));
    } catch {
      this._monitorFiltersCache = JSON.parse(JSON.stringify(DEFAULT_FILE_MONITOR_FILTERS));
      return JSON.parse(JSON.stringify(this._monitorFiltersCache));
    }
  },

  async writeMonitorFilters(config) {
    const normalized = normalizeMonitorFilters(config);
    const ok = await this.writeRawFile(
      FILE_MONITOR_FILTERS_CONFIG,
      JSON.stringify(normalized, null, 2) + "\n",
      { mode: "644" },
    );
    if (ok) {
      this._monitorFiltersCache = JSON.parse(JSON.stringify(normalized));
      if (this._mockStore) this._mockStore.monitorFilters = JSON.parse(JSON.stringify(normalized));
      await this.touchConfig();
    }
    return ok;
  },

  /** Read app config for a package */
  async readAppConfig(packageName) {
    if (!isSafePackageName(packageName)) return null;
    const path = APPS_DIR + "/" + packageName + ".json";
    const content = await this.readFile(path);
    if (!content) return null;
    try {
      return JSON.parse(content);
    } catch {
      return null;
    }
  },

  /** Write app config for a package */
  async writeAppConfig(packageName, config) {
    if (!isSafePackageName(packageName)) return false;
    const path = APPS_DIR + "/" + packageName + ".json";
    const json = JSON.stringify(config, null, 2);
    return await this.writeFile(path, json);
  },

  async writeAppConfigs(configs, options) {
    const entries = Object.entries(configs || {})
      .filter(([packageName]) => isSafePackageName(packageName))
      .sort(([left], [right]) => left.localeCompare(right));
    if (!entries.length) return false;
    const token = Date.now() + "_" + Math.floor(Math.random() * 100000);
    const stage = "/data/local/tmp/srx_bulk_apps_" + token;
    const total = entries.length;
    try {
      await this.exec(prepareManagedTempDirCommand(stage));
      for (let i = 0; i < entries.length; i += 1) {
        const [packageName, config] = entries[i];
        options?.onProgress?.(i, total, packageName);
        const ok = await this.writeRawFile(
          stage + "/" + packageName + ".json",
          JSON.stringify(config, null, 2) + "\n",
          { mode: "644" },
        );
        if (!ok) throw new Error("write staged app failed: " + packageName);
      }
      options?.onProgress?.(total, total, "");
      await this.exec(
        "stage=" +
          shellQuote(stage) +
          "; apps=" +
          shellQuote(APPS_DIR) +
          "; global=" +
          shellQuote(GLOBAL_CONFIG) +
          "; " +
          'mkdir -p "$apps" || exit 1; ' +
          'for f in "$stage"/*.json; do [ -f "$f" ] || continue; cp "$f" "$apps/${f##*/}" || exit 1; done; ' +
          'chmod 755 "$apps" || exit 1; chmod 644 "$apps"/*.json 2>/dev/null || true; ' +
          'touch "$apps" "$global" || exit 1; rm -rf "$stage"',
      );
      this.invalidateConfiguredAppsCache();
      return true;
    } catch (e) {
      try {
        await this.exec(managedTempCleanupCommand(stage));
      } catch {}
      console.error("[api] writeAppConfigs failed:", e);
      return false;
    }
  },

  /** Delete app config */
  async deleteAppConfig(packageName) {
    if (!isSafePackageName(packageName)) return false;
    const path = APPS_DIR + "/" + packageName + ".json";
    try {
      await this.exec("rm -f " + shellQuote(path));
      if (!(await this.touchConfig())) return false;
      this.invalidateConfiguredAppsCache();
      return true;
    } catch {
      return false;
    }
  },

  async readTemplates(options) {
    if (options?.force) this.invalidateTemplatesCache();
    if (this._templatesCache) return JSON.parse(JSON.stringify(this._templatesCache));
    const content = await this.readFile(TEMPLATES_CONFIG);
    if (!content) {
      this._templatesCache = [];
      return [];
    }
    try {
      const parsed = JSON.parse(content);
      const templates = Array.isArray(parsed?.templates) ? parsed.templates : [];
      this._templatesCache = templates;
      return JSON.parse(JSON.stringify(templates));
    } catch {
      this._templatesCache = [];
      return [];
    }
  },

  async writeTemplates(templates) {
    const list = Array.isArray(templates) ? templates : [];
    const ok = await this.writeRawFile(
      TEMPLATES_CONFIG,
      JSON.stringify({ templates: list }, null, 2) + "\n",
      { mode: "644" },
    );
    if (ok) {
      this._templatesCache = JSON.parse(JSON.stringify(list));
      if (this._mockStore) this._mockStore.templates = JSON.parse(JSON.stringify(list));
    }
    return ok;
  },

  async upsertTemplate(template) {
    if (!template || !template.id || !template.name || !template.config) return false;
    const templates = await this.readTemplates();
    const index = templates.findIndex((item) => item.id === template.id);
    if (index >= 0) templates[index] = template;
    else templates.push(template);
    templates.sort(
      (a, b) =>
        String(a.name || "").localeCompare(String(b.name || ""), "zh-Hans-CN") ||
        String(a.id).localeCompare(String(b.id)),
    );
    return await this.writeTemplates(templates);
  },

  async deleteTemplate(templateId) {
    const global = await this.readGlobalConfig({ force: true });
    if (global?.auto_enable_new_apps_template_id === templateId) return false;
    const templates = (await this.readTemplates({ force: true })).filter(
      (item) => item.id !== templateId,
    );
    return await this.writeTemplates(templates);
  },

  /** List all configured app package names */
  async listConfiguredApps() {
    const configs = await this.readConfiguredAppConfigs();
    return configs.map((item) => item.packageName).filter(isSafePackageName);
  },

  /** Read configured app json files in one root call. */
  async readConfiguredAppConfigs(options) {
    const now = Date.now();
    if (!options?.force && this._configuredAppsCache) {
      return this._configuredAppsCache.map((item) => ({
        packageName: item.packageName,
        config: item.config,
      }));
    }
    try {
      const marker = "__SRX_APP_CONFIG__";
      const out = await this.exec(
        "mkdir -p " +
          shellQuote(APPS_DIR) +
          "; " +
          "for f in " +
          shellQuote(APPS_DIR) +
          "/*.json; do " +
          '[ -f "$f" ] || continue; b="${f##*/}"; p="${b%.json}"; ' +
          'printf "\\n' +
          marker +
          '%s\\n" "$p"; cat "$f"; printf "\\n"; done 2>/dev/null',
      );
      const items = [];
      const lines = String(out || "").split("\n");
      let current = null;
      let body = [];
      const flush = () => {
        if (!current || !isSafePackageName(current)) return;
        let config = null;
        const text = body.join("\n").trim();
        if (text) {
          try {
            config = JSON.parse(text);
          } catch {
            config = null;
          }
        }
        items.push({ packageName: current, config });
      };
      for (let i = 0; i < lines.length; i += 1) {
        const line = lines[i];
        if (line.startsWith(marker)) {
          flush();
          current = line.slice(marker.length).trim();
          body = [];
        } else if (current) {
          body.push(line);
        }
        if (i > 0 && i % 250 === 0) await yieldToUi();
      }
      flush();
      this._configuredAppsCache = items;
      this._configuredAppsCacheAt = now;
      return items.map((item) => ({ packageName: item.packageName, config: item.config }));
    } catch {
      return [];
    }
  },

  /** List Android user ids available on device. */
  async listUsers() {
    try {
      const out = await this.exec("cmd user list 2>/dev/null || pm list users 2>/dev/null");
      const ids = [];
      String(out || "").replace(/UserInfo\{([0-9]+):/g, (_, id) => {
        ids.push(id);
        return "";
      });
      if (!ids.length)
        String(out || "").replace(/\{([0-9]+):/g, (_, id) => {
          ids.push(id);
          return "";
        });
      return Array.from(new Set(ids.length ? ids : ["0"]));
    } catch {
      return ["0"];
    }
  },

  /** Get list of installed apps (user + system) */
  async getInstalledApps(userId) {
    const safeUser = isSafeUserId(userId) ? String(userId) : "";
    const fromKsu = this.getInstalledAppsFromKsu();
    if (fromKsu.length) return fromKsu;
    const fromDex = await this.getInstalledAppsFromDex(safeUser);
    if (fromDex.length) return fromDex;
    return await this.getInstalledAppsFromPackageManager(safeUser);
  },

  async getInstalledAppsFromPackageManager(userId) {
    const safeUser = isSafeUserId(userId) ? String(userId) : "";
    const userArg = safeUser ? " --user " + safeUser : "";
    const commands = [
      "pm list packages -f" + userArg + " 2>/dev/null",
      "cmd package list packages -f" + userArg + " 2>/dev/null",
    ];
    for (const command of commands) {
      try {
        const out = await this.exec(command + ' | sed "s/package://" | awk -F= "{print \$2}"');
        const packages = out.split("\n").filter(Boolean).filter(isSafePackageName);
        if (packages.length) return packages;
      } catch {}
    }
    return [];
  },
  async prepareCompatAppList(userId, force) {
    if (typeof ksu !== "undefined" && ksu.listPackages) return false;
    const safeUser = isSafeUserId(userId) ? String(userId) : "0";
    const cacheKey = safeUser || "0";
    if (!force && this._compatAppListPrepared.has(cacheKey)) return true;
    const apps = await this.getInstalledAppsFromDex(safeUser, { force: true });
    if (apps.length) {
      this._compatAppListPrepared.add(cacheKey);
      return true;
    }
    return false;
  },

  getInstalledAppsFromKsu() {
    if (typeof ksu === "undefined" || !ksu.listPackages) return [];
    try {
      const userRaw = normalizePackageResult(ksu.listPackages("user"));
      const systemRaw = normalizePackageResult(ksu.listPackages("system"));
      const rememberInfo = (item, isSystem) => {
        const pkg = getPackageNameFromInfo(item);
        if (!isSafePackageName(pkg)) return;
        if (item && typeof item === "object") {
          item.isSystem = item.isSystem ?? isSystem;
          this._packageInfo.set(pkg, item);
        } else {
          this._packageInfo.set(pkg, { packageName: pkg, isSystem });
        }
      };
      userRaw.forEach((item) => rememberInfo(item, false));
      systemRaw.forEach((item) => rememberInfo(item, true));
      [...userRaw, ...systemRaw].forEach((item) => {
        if (item && typeof item === "object") {
          const pkg = getPackageNameFromInfo(item);
          if (isSafePackageName(pkg)) this._packageInfo.set(pkg, item);
        }
      });
      const names = Array.from(
        new Set(
          [...userRaw, ...systemRaw]
            .map((item) => {
              return getPackageNameFromInfo(item);
            })
            .filter(isSafePackageName),
        ),
      );
      this.populatePackageInfo(names);
      return names;
    } catch {
      return [];
    }
  },

  populatePackageInfo(packageNames) {
    if (typeof ksu === "undefined" || !ksu.getPackagesInfo || !packageNames.length) return;
    try {
      let info = [];
      try {
        info = normalizePackageResult(ksu.getPackagesInfo(packageNames));
      } catch {}
      if (!info.length) {
        try {
          info = normalizePackageResult(ksu.getPackagesInfo(JSON.stringify(packageNames)));
        } catch {}
      }
      info.forEach((item) => {
        const pkg = item.packageName || item.package || "";
        if (isSafePackageName(pkg)) this._packageInfo.set(pkg, item);
      });
    } catch {}
  },

  async getInstalledAppsFromDex(userId, options) {
    const safeUser = isSafeUserId(userId) ? String(userId) : "0";
    const cacheKey = safeUser || "0";
    if (!options?.force && this._compatAppListCache.has(cacheKey)) {
      return this._compatAppListCache.get(cacheKey).slice();
    }
    try {
      await this.exec(
        "mkdir -p /data/Namespace-Proxy; if [ -f " +
          shellQuote(LIST_APPS_DEX) +
          " ]; then /system/bin/app_process64 -Djava.class.path=" +
          shellQuote(LIST_APPS_DEX) +
          " / Main --user " +
          safeUser +
          " > " +
          shellQuote(LIST_APPS_OUTPUT) +
          " 2>/dev/null; fi",
      );
      const out = await this.readFile(LIST_APPS_OUTPUT);
      const apps = parseCompatAppList(out).map((item) => {
        this._packageInfo.set(item.packageName, item);
        return item.packageName;
      });
      this._compatAppListCache.set(cacheKey, apps);
      this._compatAppListPrepared.add(cacheKey);
      return apps.slice();
    } catch {
      return [];
    }
  },

  /** Get app label for a package */
  async getAppLabel(packageName) {
    if (!isSafePackageName(packageName)) return packageName;
    const cached = this._packageInfo.get(packageName);
    if (cached) return cached.appLabel || cached.label || cached.name || packageName;
    try {
      return await this.exec(
        "pm dump " +
          shellQuote(packageName) +
          " 2>/dev/null | " +
          'grep -A1 "application:" | tail -1 | ' +
          'sed "s/.*labelRes=0x[0-9a-fA-F]* //" | sed "s/^[ \t]*//"',
      );
    } catch {
      return packageName;
    }
  },

  getCachedAppInfo(packageName) {
    return this._packageInfo.get(packageName) || null;
  },

  getAppIconSrc(packageName) {
    if (!isSafePackageName(packageName)) return "";
    if (typeof ksu !== "undefined" && ksu.listPackages) return "ksu://icon/" + packageName;
    return "";
  },

  /** Get app icon path */
  async getAppIconPath(packageName) {
    if (!isSafePackageName(packageName)) return "";
    return await this.exec(
      "pm path " + shellQuote(packageName) + ' 2>/dev/null | head -1 | sed "s/package://"',
    );
  },

  /** List directories at a path */
  async listDir(dirPath) {
    try {
      const out = await this.exec("ls -1Ap " + shellQuote(dirPath) + " 2>/dev/null");
      if (!out) return { dirs: [], files: [] };
      const entries = out
        .split("\n")
        .filter(
          (entry) => entry && entry !== "./" && entry !== "../" && entry !== "." && entry !== "..",
        );
      const dirs = [];
      const files = [];
      for (const entry of entries) {
        if (entry.endsWith("/")) {
          dirs.push(entry.slice(0, -1));
        } else {
          files.push(entry);
        }
      }
      return { dirs, files };
    } catch {
      return { dirs: [], files: [] };
    }
  },

  /** Check module status */
  async getModuleStatus() {
    try {
      const out = await this.exec(
        withSrxCtlFallback(
          "status",
          "boot_id=$(cat /proc/sys/kernel/random/boot_id 2>/dev/null); " +
            "boot_ok=$(cat " +
            shellQuote(MODULE_BOOT_OK) +
            " 2>/dev/null); " +
            "boot_marker=" +
            shellQuote(LOGS_DIR) +
            "/boot_${boot_id}.marker; " +
            "if [ ! -d " +
            shellQuote(MODULE_DIR) +
            " ]; then echo unknown; " +
            "elif [ -f " +
            shellQuote(RUNTIME_DISABLE) +
            " ] || [ -f " +
            shellQuote(MODULE_DISABLE) +
            " ]; then echo disabled; " +
            'elif [ -n "$boot_id" ] && { [ "$boot_ok" = "$boot_id" ] || [ -f "$boot_marker" ]; }; then echo enabled; ' +
            "else echo reboot_required; fi",
        ),
      );
      return ["enabled", "disabled", "reboot_required", "unknown"].includes(out) ? out : "unknown";
    } catch {
      return "unknown";
    }
  },

  async ensureLogCollectors() {
    try {
      if (await this.fileExists(SRXCTL)) {
        await this.exec(srxCtlCommand("ensure-collectors"));
      }
    } catch {}
  },

  /** Count configured apps */
  async getConfiguredAppCount() {
    const apps = await this.listConfiguredApps();
    return apps.length;
  },

  async getEnabledAppCount() {
    const apps = await this.readConfiguredAppConfigs();
    let count = 0;
    apps.forEach((item) => {
      const cfg = item.config;
      const users = (cfg && cfg.users) || {};
      const enabled = Object.values(users).some((profile) => profile && profile.enabled !== false);
      if (enabled) count += 1;
    });
    return count;
  },

  async getModuleVersion() {
    try {
      const out = await this.exec(
        'sed -n "s/^version=//p" ' + shellQuote(MODULE_PROP) + " 2>/dev/null | head -n 1",
      );
      return out || "";
    } catch {
      return "";
    }
  },

  getReleaseRepository() {
    return DEFAULT_RELEASE_REPOSITORY;
  },

  getOfficialReleaseRepository() {
    return DEFAULT_OFFICIAL_RELEASE_REPOSITORY;
  },

  getReleaseRepositoryUrl() {
    return "https://github.com/" + DEFAULT_RELEASE_REPOSITORY;
  },

  getOfficialReleaseRepositoryUrl() {
    return "https://github.com/" + DEFAULT_OFFICIAL_RELEASE_REPOSITORY;
  },

  getUpdateManifestUrl() {
    return UPDATE_MANIFEST_URL;
  },

  async fetchUpdateManifest(manifestUrl) {
    if (!hasNativeWebUiBridge()) return mockUpdateManifest();
    const url = manifestUrl || UPDATE_MANIFEST_URL;
    const response = await fetch(url, {
      method: "GET",
      cache: "no-store",
      headers: { Accept: "application/json" },
    });
    if (!response.ok) {
      if (response.status === 403)
        throw new Error("更新清单访问被 GitHub 限制：HTTP 403，请稍后或切换网络");
      if (response.status === 404)
        throw new Error("更新清单不存在：HTTP 404，请确认仓库分支已提交 update.json");
      throw new Error("更新清单响应异常：HTTP " + response.status);
    }
    return await response.json();
  },

  async checkForUpdates(options) {
    const manifest = await this.fetchUpdateManifest(options?.manifestUrl);
    return findReleaseUpdate(
      manifest,
      options?.repository || DEFAULT_RELEASE_REPOSITORY,
      options?.currentVersionName || "",
      options?.channel || "Stable",
    );
  },

  /** Touch config to trigger hot reload */
  async touchConfig() {
    try {
      await this.exec(
        "mkdir -p " +
          shellQuote(APPS_DIR) +
          " && touch " +
          shellQuote(APPS_DIR) +
          " 2>/dev/null && touch " +
          shellQuote(GLOBAL_CONFIG) +
          " 2>/dev/null",
      );
      return true;
    } catch {
      return false;
    }
  },

  async notifyRuntimeConfigChanged() {
    await this.touchConfig();
    await this.ensureLogCollectors();
    try {
      await this.exec(
        "for uri in content://media/external/file content://media/internal/file; do " +
          'content query --uri "$uri" --projection _id --limit 1 >/dev/null 2>&1 || true; ' +
          "done",
      );
    } catch {}
    return true;
  },

  /** Atomically replace global.json and apps/*.json from a validated backup snapshot. */
  async restoreConfigSnapshot(snapshot) {
    const apps = snapshot?.apps || {};
    const globalConfig = snapshot?.global || DEFAULT_GLOBAL_CONFIG;
    const templates = Array.isArray(snapshot?.templates) ? snapshot.templates : [];
    const monitorFilters = snapshot?.monitor_filters
      ? normalizeMonitorFilters(snapshot.monitor_filters)
      : JSON.parse(JSON.stringify(DEFAULT_FILE_MONITOR_FILTERS));
    const token = Date.now() + "_" + Math.floor(Math.random() * 100000);
    const stage = "/data/local/tmp/srx_restore_stage_" + token;
    const rollback = "/data/local/tmp/srx_restore_rollback_" + token;
    const stageApps = stage + "/apps";
    try {
      await this.exec(
        managedTempCleanupCommand(stage, rollback) + "; mkdir -p " + shellQuote(stageApps),
      );
      const globalOk = await this.writeRawFile(
        stage + "/global.json",
        JSON.stringify(globalConfig, null, 2) + "\n",
        { mode: "644" },
      );
      if (!globalOk) throw new Error("write staged global failed");
      const templatesOk = await this.writeRawFile(
        stage + "/templates.json",
        JSON.stringify({ templates }, null, 2) + "\n",
        { mode: "644" },
      );
      if (!templatesOk) throw new Error("write staged templates failed");
      const filtersOk = await this.writeRawFile(
        stage + "/file_monitor_filters.json",
        JSON.stringify(monitorFilters, null, 2) + "\n",
        { mode: "644" },
      );
      if (!filtersOk) throw new Error("write staged monitor filters failed");
      const entries = Object.entries(apps)
        .filter(([packageName]) => isSafePackageName(packageName))
        .sort(([left], [right]) => left.localeCompare(right));
      for (const [packageName, config] of entries) {
        const ok = await this.writeRawFile(
          stageApps + "/" + packageName + ".json",
          JSON.stringify(config, null, 2) + "\n",
          { mode: "644" },
        );
        if (!ok) throw new Error("write staged app failed: " + packageName);
      }
      await this.exec(
        "config=" +
          shellQuote(CONFIG_DIR) +
          "; stage=" +
          shellQuote(stage) +
          "; rollback=" +
          shellQuote(rollback) +
          "; " +
          'restore_prev() { rm -rf "$config/apps"; if [ -d "$rollback/apps" ]; then mv "$rollback/apps" "$config/apps"; else mkdir -p "$config/apps"; fi; rm -f "$config/global.json" "$config/templates.json" "$config/file_monitor_filters.json"; if [ -f "$rollback/global.json" ]; then mv "$rollback/global.json" "$config/global.json"; fi; if [ -f "$rollback/templates.json" ]; then mv "$rollback/templates.json" "$config/templates.json"; fi; if [ -f "$rollback/file_monitor_filters.json" ]; then mv "$rollback/file_monitor_filters.json" "$config/file_monitor_filters.json"; fi; }; ' +
          'fail_restore() { rc=$?; restore_prev; rm -rf "$stage" "$rollback"; exit $rc; }; ' +
          'mkdir -p "$config" "$rollback" || exit 1; ' +
          'if [ -d "$config/apps" ]; then mv "$config/apps" "$rollback/apps" || fail_restore; fi; ' +
          'if [ -f "$config/global.json" ]; then mv "$config/global.json" "$rollback/global.json" || fail_restore; fi; ' +
          'if [ -f "$config/templates.json" ]; then mv "$config/templates.json" "$rollback/templates.json" || fail_restore; fi; ' +
          'if [ -f "$config/file_monitor_filters.json" ]; then mv "$config/file_monitor_filters.json" "$rollback/file_monitor_filters.json" || fail_restore; fi; ' +
          'mv "$stage/apps" "$config/apps" || fail_restore; mv "$stage/global.json" "$config/global.json" || fail_restore; mv "$stage/templates.json" "$config/templates.json" || fail_restore; mv "$stage/file_monitor_filters.json" "$config/file_monitor_filters.json" || fail_restore; ' +
          'chmod 755 "$config" "$config/apps" || fail_restore; chmod 644 "$config/global.json" "$config/templates.json" "$config/file_monitor_filters.json" || fail_restore; chmod 644 "$config/apps"/*.json 2>/dev/null || true; ' +
          'touch "$config/apps" "$config/global.json" "$config/templates.json" "$config/file_monitor_filters.json" || fail_restore; rm -rf "$rollback" "$stage"',
      );
      this._globalConfigCache = Object.assign({}, globalConfig);
      this._templatesCache = JSON.parse(JSON.stringify(templates));
      this._monitorFiltersCache = JSON.parse(JSON.stringify(monitorFilters));
      this.invalidateConfiguredAppsCache();
      await this.touchConfig();
      await this.ensureLogCollectors();
      return true;
    } catch (e) {
      try {
        await this.exec(managedTempCleanupCommand(stage, rollback));
      } catch {}
      console.error("[api] restoreConfigSnapshot failed:", e);
      return false;
    }
  },

  async stopModule() {
    const beforePids = await this.getMediaProviderPids();
    await this.exec(
      withSrxCtlFallback(
        "stop",
        "mkdir -p " +
          shellQuote(CONFIG_DIR) +
          " && touch " +
          shellQuote(RUNTIME_DISABLE) +
          ' && printf "{\\"runtime_disabled\\":true}\\n" > ' +
          shellQuote(RUNTIME_STATE_CONFIG),
      ),
    );
    return await this.waitForMediaProviderRestart(beforePids, {
      timeoutMs: 10000,
      intervalMs: 250,
    });
  },

  async startModule() {
    const beforePids = await this.getMediaProviderPids();
    await this.exec(
      withSrxCtlFallback(
        "start",
        "mkdir -p " +
          shellQuote(CONFIG_DIR) +
          " " +
          shellQuote(LOGS_DIR) +
          " && " +
          "rm -f " +
          shellQuote(RUNTIME_DISABLE) +
          " && " +
          'printf "{\\"runtime_disabled\\":false}\\n" > ' +
          shellQuote(RUNTIME_STATE_CONFIG) +
          " && " +
          "daemon=" +
          shellQuote(MODULE_DIR + "/bin/srx_daemon") +
          "; pidfile=" +
          shellQuote(LOGS_DIR + "/.srx_daemon.pid") +
          "; " +
          'if [ -x "$daemon" ]; then old=$(cat "$pidfile" 2>/dev/null); if [ -z "$old" ] || ! kill -0 "$old" 2>/dev/null; then "$daemon" >/dev/null 2>&1 & echo $! > "$pidfile"; chmod 600 "$pidfile" 2>/dev/null; fi; fi',
      ),
    );
    return await this.waitForMediaProviderRestart(beforePids, {
      timeoutMs: 10000,
      intervalMs: 250,
    });
  },

  async restartMediaProvider() {
    const beforePids = await this.getMediaProviderPids();
    await this.exec(
      withSrxCtlFallback(
        "restart-media",
        'for p in com.android.providers.media.module com.google.android.providers.media.module com.android.providers.media; do pids=$(pidof "$p" 2>/dev/null); for pid in $pids; do kill -9 "$pid" 2>/dev/null || true; done; done; ' +
          "content query --uri content://media/external/file --projection _id --limit 1 >/dev/null 2>&1 || true; content query --uri content://media/internal/file --projection _id --limit 1 >/dev/null 2>&1 || true",
      ),
    );
    const ok = await this.waitForMediaProviderRestart(beforePids, {
      timeoutMs: 8000,
      intervalMs: 250,
    });
    await this.ensureLogCollectors();
    return ok;
  },

  async getMediaProviderPids() {
    try {
      const packages = MEDIA_PROVIDER_PACKAGES.map(shellQuote).join(" ");
      const command = "for p in " + packages + '; do pidof "$p" 2>/dev/null || true; done';
      const out = await this.exec(command);
      return String(out || "")
        .split(/\s+/)
        .filter(Boolean);
    } catch {
      return [];
    }
  },

  async waitForMediaProviderRestart(beforePids, options) {
    const oldPids = new Set((beforePids || []).map(String));
    const timeoutMs = options?.timeoutMs || 15000;
    const intervalMs = options?.intervalMs || 500;
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      const current = await this.getMediaProviderPids();
      const hasPid = current.length > 0;
      const hasNewPid = current.some((pid) => !oldPids.has(String(pid)));
      if (hasPid && (!oldPids.size || hasNewPid)) return true;
      await new Promise((resolve) => setTimeout(resolve, intervalMs));
    }
    return false;
  },

  showManagerToast(message) {
    try {
      if (typeof ksu !== "undefined") {
        if (typeof ksu.toast === "function") {
          ksu.toast(message);
          return true;
        }
        if (typeof ksu.showToast === "function") {
          ksu.showToast(message);
          return true;
        }
      }
    } catch {}
    if (window.Theme?.showToast) Theme.showToast(message, "success");
    return true;
  },

  // ── Mock for development ──
  _mockStore: {
    global: {
      file_monitor_enabled: true,
      fuse_fix_enabled: true,
      verbose_logging_enabled: false,
      auto_enable_redirect_for_new_apps: false,
      auto_enable_new_apps_template_id: "",
      app_config_auto_save: false,
    },
    monitorFilters: JSON.parse(JSON.stringify(DEFAULT_FILE_MONITOR_FILTERS)),
    fileMonitorLogCleared: false,
    statsCount: 2,
    templates: [],
    apps: {
      "com.tencent.mm": {
        users: {
          0: {
            enabled: true,
            mapping_mode_only: false,
            allowed_real_paths: ["Pictures", "Download/WeChat"],
            excluded_real_paths: ["!Pictures/Private"],
            sandboxed_paths: [".xlDownload"],
            read_only_paths: ["Pictures"],
            path_mappings: {
              "Download/WeChat": "Download/WeChat_Redirect",
            },
          },
        },
      },
      "com.autonavi.minimap": {
        users: {
          0: {
            enabled: true,
            mapping_mode_only: false,
            allowed_real_paths: ["Download/amap"],
            read_only_paths: [],
            path_mappings: {},
          },
        },
      },
    },
  },

  _mockFileMonitorLog() {
    if (this._mockStore.fileMonitorLogCleared) return "";
    return [
      "2026-05-25 16:05:12|com.tencent.mm|com.tencent.mm|CREATE|/storage/emulated/0/Download/WeChat/pass.jpg|op=inotify|ret=0|errno=0|identify_method=daemon_inotify|identify_reliability=medium|source=allowed_real_path|backend=/data/media/0/Download/WeChat/pass.jpg",
      "2026-05-25 16:06:12|com.tencent.mm|com.tencent.mm|CREATE|/storage/emulated/0/Download/WeChat_Redirect/a.jpg|op=inotify|ret=0|errno=0|identify_method=daemon_inotify|identify_reliability=medium|source=path_mapping|backend=/data/media/0/Download/WeChat_Redirect/a.jpg|from=/storage/emulated/0/Download/WeChat/a.jpg",
      "2026-05-25 16:07:08|com.autonavi.minimap|com.autonavi.minimap|CREATE|/storage/emulated/0/Download/amap/cache.dat|op=inotify|ret=0|errno=0|identify_method=daemon_inotify|identify_reliability=medium|source=sandbox_path|backend=/data/media/0/Android/data/com.autonavi.minimap/sdcard/Download/amap/cache.dat",
      "2026-05-25 16:08:14|com.tencent.mm|com.tencent.mm|RENAME|/storage/emulated/0/Download/WeChat/final.jpg|ret=0|errno=0|identify_method=caller|identify_reliability=high|op=rename|from=/storage/emulated/0/Download/WeChat/tmp.jpg",
      "2026-05-25 16:09:21|com.tencent.mm|com.tencent.mm|UNLINK|/storage/emulated/0/Download/WeChat/missing.jpg|ret=-1|errno=13|identify_method=caller|identify_reliability=high|op=unlink",
    ].join("\n");
  },

  async _mockExec(cmd) {
    if (cmd.includes(FILE_MONITOR_LOG) && (cmd.includes(": >") || cmd.includes('echo "" >'))) {
      this._mockStore.fileMonitorLogCleared = true;
      return "";
    }
    if (cmd.includes("__SRX_APP_CONFIG__")) {
      return Object.entries(this._mockStore.apps)
        .map(
          ([packageName, config]) =>
            "\n__SRX_APP_CONFIG__" + packageName + "\n" + JSON.stringify(config),
        )
        .join("\n");
    }
    // Simulate package list
    if (cmd.includes("pm list packages")) {
      return (
        Object.keys(this._mockStore.apps).join("\n") +
        "\ncom.android.chrome\ncom.twitter.android\ncom.spotify.music"
      );
    }
    if (cmd.includes("tail") && cmd.includes(FILE_MONITOR_LOG)) {
      return this._mockFileMonitorLog();
    }
    // Simulate cat
    if (cmd.includes("cat")) {
      const match = cmd.match(/["']([^"']+)["']/);
      if (match) {
        const path = match[1];
        if (path === GLOBAL_CONFIG) return JSON.stringify(this._mockStore.global);
        if (path === FILE_MONITOR_FILTERS_CONFIG)
          return JSON.stringify(this._mockStore.monitorFilters || DEFAULT_FILE_MONITOR_FILTERS);
        if (path === TEMPLATES_CONFIG)
          return JSON.stringify({ templates: this._mockStore.templates || [] });
        if (path === MODULE_STATS_FILE) return String(this._mockStore.statsCount || 0);
        if (path === FILE_MONITOR_LOG) {
          return this._mockFileMonitorLog();
        }
        const appMatch = path.match(/apps\/(.+)\.json/);
        if (appMatch) {
          const cfg = this._mockStore.apps[appMatch[1]];
          return cfg ? JSON.stringify(cfg) : "";
        }
      }
    }
    if (cmd.includes("module.prop") && cmd.includes("version=")) return "v1.2.45";
    // Simulate ls apps
    if (cmd.includes("ls") && cmd.includes(APPS_DIR)) {
      return Object.keys(this._mockStore.apps).join(".json\n") + ".json";
    }
    if (cmd.includes("cmd user list")) return "0\n10";
    // Simulate module status
    if (cmd.includes("disable")) return "enabled";
    // Simulate app label
    if (cmd.includes("pm dump")) return "Mock App";
    return "";
  },
};

// Export for use
window.Api = Api;
