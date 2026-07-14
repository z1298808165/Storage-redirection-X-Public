const fs = require("fs");
const path = require("path");
const vm = require("vm");

const repoRoot = path.resolve(__dirname, "..");
const apiJsPath = path.join(repoRoot, "assets/zygisk_module/webroot/js/api.js");
const appJsPath = path.join(repoRoot, "assets/zygisk_module/webroot/js/app.js");
const fixtureDir = path.join(repoRoot, "docs/config-fixtures");

function readJson(name) {
  return JSON.parse(fs.readFileSync(path.join(fixtureDir, name), "utf8"));
}

function stable(value) {
  if (Array.isArray(value)) return value.map(stable);
  if (value && typeof value === "object") {
    return Object.keys(value)
      .sort()
      .reduce((out, key) => {
        out[key] = stable(value[key]);
        return out;
      }, {});
  }
  return value;
}

function assertDeepEqual(actual, expected, label) {
  const actualText = JSON.stringify(stable(actual));
  const expectedText = JSON.stringify(stable(expected));
  if (actualText !== expectedText) {
    throw new Error(`${label} mismatch\nactual:   ${actualText}\nexpected: ${expectedText}`);
  }
}

const sandbox = {
  AbortController,
  console,
  URL,
  setTimeout: () => 0,
  clearTimeout: () => {},
  localStorage: { getItem: () => null, setItem: () => {} },
  window: {
    __SRX_ENABLE_TEST_EXPORTS__: true,
    App: {},
    matchMedia: () => ({ matches: false }),
    addEventListener: () => {},
    history: { pushState: () => {}, replaceState: () => {} },
    location: { hash: "" },
  },
  document: {
    readyState: "loading",
    addEventListener: () => {},
    querySelector: () => null,
    querySelectorAll: () => [],
    createElement: () => ({ textContent: "", innerHTML: "" }),
    documentElement: { setAttribute: () => {} },
  },
};
sandbox.window.window = sandbox.window;
sandbox.window.document = sandbox.document;
sandbox.globalThis = sandbox;

vm.runInNewContext(fs.readFileSync(apiJsPath, "utf8"), sandbox, { filename: apiJsPath });
vm.runInNewContext(fs.readFileSync(appJsPath, "utf8"), sandbox, { filename: appJsPath });

const webui = sandbox.window.__SRX_WEBUI_TEST__;
if (!webui) throw new Error("WebUI test exports were not initialized");

assertDeepEqual(
  sandbox.window.Api.getOfficialReleaseRepositoryUrl(),
  "https://github.com/Kindness-Kismet/Storage-redirection-X-Public",
  "official-release-repository",
);
assertDeepEqual(webui.updateChannelBadge("Stable", false), "正式版", "stable-update-badge");
assertDeepEqual(webui.updateChannelBadge("Beta", true), "测试版", "beta-update-badge");
assertDeepEqual(
  webui.updateVersionBadge({
    versionName: "1.2.57-ci.467",
    tagName: "ci-build-29184509526-1",
  }),
  "1.2.57-ci.467",
  "update-version-prefers-version-name",
);
assertDeepEqual(
  webui.updateVersionBadge({ tagName: "v1.2.57" }),
  "v1.2.57",
  "update-version-falls-back-to-tag",
);
const toggleClasses = new Set();
const toggleAttributes = {};
const toggle = {
  disabled: false,
  classList: {
    toggle: (name, enabled) => {
      if (enabled) toggleClasses.add(name);
      else toggleClasses.delete(name);
    },
  },
  setAttribute: (name, value) => {
    toggleAttributes[name] = value;
  },
};
webui.setToggleState(toggle, true);
webui.setToggleBusy(toggle, true);
assertDeepEqual(toggleClasses.has("on"), true, "toggle-visual-state");
assertDeepEqual(toggleAttributes["aria-checked"], "true", "toggle-accessible-state");
assertDeepEqual(toggle.disabled, true, "toggle-disabled-while-saving");
assertDeepEqual(toggleAttributes["aria-busy"], "true", "toggle-accessible-busy-state");

const rawApp = readJson("app-profile-normalization-input.json");
const normalizedApp = readJson("app-profile-normalization-output.json");

const normalizedResult = webui.normalizeBackupAppConfig("com.example", rawApp);
if (!normalizedResult.config) {
  throw new Error(`raw app fixture was rejected: ${normalizedResult.warnings.join("; ")}`);
}
assertDeepEqual(normalizedResult.config, normalizedApp, "app-profile-normalization-input");

const result = webui.normalizeBackupAppConfig("com.example", normalizedApp);
if (!result.config) {
  throw new Error(`normalized app fixture was rejected: ${result.warnings.join("; ")}`);
}
assertDeepEqual(result.config, normalizedApp, "app-profile-normalization-output");

const selfPackageResult = webui.normalizeBackupAppConfig("com.storage.redirect.x", normalizedApp);
if (!selfPackageResult.config) {
  throw new Error(`self package app config was rejected: ${selfPackageResult.warnings.join("; ")}`);
}

assertDeepEqual(
  webui.normalizeBackupUiPreferences({
    predictive_back: true,
    floating_bottom_bar: false,
    liquid_glass: true,
    blur_effect: false,
    dynamic_color: true,
    accent_color: 0xff2196f3,
    color_style: "vibrant",
    color_spec: "Spec2021",
    theme_mode: "dark",
    page_scale: 1.25,
    auto_check_updates: false,
    update_channel: "Beta",
  }),
  {
    predictive_back: true,
    floating_bottom_bar: false,
    liquid_glass: true,
    blur_effect: false,
    dynamic_color: true,
    accent_color: -14575885,
    color_style: "Vibrant",
    color_spec: "Spec2021",
    theme_mode: "Dark",
    page_scale: 1.1,
    auto_check_updates: false,
    update_channel: "Beta",
  },
  "backup-ui-preferences",
);
assertDeepEqual(
  webui.normalizeBackupUiPreferences({
    update_channel: "BadChannel",
    color_style: "BadStyle",
    accent_color: 4294967296,
  }),
  null,
  "backup-ui-preferences-invalid-values",
);

const createOpenEntry = webui.parseLogLine(
  `2026-07-12 12:34:56|com.example|com.example|OPEN|/storage/emulated/0/Download/a.txt|ret=8|errno=0|op=openat2|op_filter=openat2:create|flags=0x40`,
);
assertDeepEqual(createOpenEntry.operationLabel, `openat2`, `log-operation-keeps-raw-type`);
assertDeepEqual(createOpenEntry.operationIntent, `create`, `log-operation-exposes-intent`);
assertDeepEqual(
  webui.logOperationCopyValue(createOpenEntry),
  `openat2:create`,
  `log-operation-copy-uses-filter-rule`,
);
assertDeepEqual(
  webui.shouldFilterMonitorLogEntry(createOpenEntry, {
    excluded_paths: [],
    excluded_operations: [`*:create`],
  }),
  true,
  `log-create-intent-filter`,
);
assertDeepEqual(
  webui.shouldFilterMonitorLogEntry(createOpenEntry, {
    excluded_paths: [],
    excluded_operations: [`open*:read`],
  }),
  false,
  `log-read-rule-does-not-hide-create-intent`,
);
assertDeepEqual(
  webui.splitMonitorOperationRules([`open*:read`, `*:create`, `rename*`]),
  { operations: [`open*:read`, `rename*`], intents: [`create`] },
  `monitor-filter-splits-intents`,
);
assertDeepEqual(
  webui.normalizeBackupMonitorFilters({
    excluded_paths: [`Pictures`, `Download`, `Android/media`],
    excluded_operations: [`rename*`, `*:create`, `open*:read`],
  }),
  {
    excluded_paths: [`Android/media`, `Download`, `Pictures`],
    excluded_operations: [`*:create`, `open*:read`, `rename*`],
  },
  `monitor-filter-sorts-rules-alphabetically`,
);

(async () => {
  sandbox.window.LSPosedBridge = { exec: () => "" };
  const manifestRequests = [];
  sandbox.fetch = async (url) => {
    manifestRequests.push(url);
    if (url.includes("raw.githubusercontent.com")) return { ok: false, status: 404 };
    return { ok: true, status: 200, json: async () => ({ schema: 1 }) };
  };
  const fetchedManifest = await sandbox.window.Api.fetchUpdateManifest(
    "https://raw.githubusercontent.com/example/repo/SRX-R/update.json",
  );
  assertDeepEqual(fetchedManifest, { schema: 1 }, "update-manifest-fallback-result");
  assertDeepEqual(
    manifestRequests,
    [
      "https://raw.githubusercontent.com/example/repo/SRX-R/update.json",
      "https://cdn.jsdelivr.net/gh/example/repo@SRX-R/update.json",
    ],
    "update-manifest-fallback-order",
  );

  sandbox.window.Api.exec = async () =>
    [
      "system_accent1_600=#ff336699",
      "system_accent1_200=0xff99CCEE",
      "system_accent2_600=#ff445566",
      "system_accent2_200=#ffAABBCC",
    ].join("\n");
  const palette = await sandbox.window.Api.readSystemAccentPalette();
  assertDeepEqual(
    palette,
    {
      lightPrimary: "#336699",
      darkPrimary: "#99CCEE",
      lightSecondary: "#445566",
      darkSecondary: "#AABBCC",
    },
    "webui-system-accent-palette",
  );
  console.log("WebUI config fixtures verified");
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
