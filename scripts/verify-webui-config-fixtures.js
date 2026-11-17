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
    createElement: () => ({
      textContent: "",
      get innerHTML() {
        return this.textContent.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
      },
    }),
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
assertDeepEqual(
  sandbox.window.Api.parseRuntimeActivationCount(
    "schema=2\nruntime_activations=18446744073709551615\n",
  ),
  "18446744073709551615",
  "runtime-activation-count-preserves-u64",
);
assertDeepEqual(
  sandbox.window.Api.parseRuntimeActivationCount("1234\n"),
  "0",
  "runtime-activation-count-rejects-legacy-format",
);
assertDeepEqual(
  sandbox.window.Api.parseRuntimeActivationCount("schema=2\nruntime_activations=invalid\n"),
  "0",
  "runtime-activation-count-rejects-invalid-value",
);
assertDeepEqual(
  sandbox.window.Api.parseRuntimeActivationCount(
    "schema=2\nruntime_activations=18446744073709551616\n",
  ),
  "0",
  "runtime-activation-count-rejects-u64-overflow",
);
assertDeepEqual(
  ["999", "1000", "1200", "10000", "999950", "100000000", "1000000000"].map(
    sandbox.window.Api.formatCompactRuntimeActivationCount,
  ),
  ["999", "1K", "1.2K", "10K", "1M", "100M", "1B"],
  "runtime-activation-count-uses-token-style-units",
);
assertDeepEqual(
  sandbox.window.Api.formatCompactRuntimeActivationCount("18446744073709551615"),
  "18.4Qi",
  "runtime-activation-count-formats-u64-max",
);
assertDeepEqual(
  sandbox.window.Api.formatCompactRuntimeActivationCount("18446744073709551616"),
  "0",
  "runtime-activation-count-compact-rejects-u64-overflow",
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
const identityBase = "2026-11-16 12:34:01|com.android.providers.media.module|";
for (const name of ["复制+worker", "worker!", "a|ret=0\r\n%7C"]) {
  const encoded = name
    .replace(/%/g, "%25")
    .replace(/\|/g, "%7C")
    .replace(/\r/g, "%0D")
    .replace(/\n/g, "%0A");
  const entry = webui.parseLogLine(
    `2026-11-16 00:00:00|${encoded}|${encoded}|OPEN|/storage/emulated/0/Download/test|field_encoding=percent|ret=-1|errno=13|event_role=request|op=open:write|identify_method=executor|caller_boundary=root|caller_uid=0|native=${encoded}`,
  );
  assertDeepEqual(entry.processPkg, name, "executor-process-round-trip");
  assertDeepEqual(entry.callerPkg, name, "executor-caller-round-trip");
  assertDeepEqual(entry.extras.native, name, "executor-native-round-trip");
  assertDeepEqual(entry.hasApplicationIdentity, false, "executor-is-not-an-app");
  assertDeepEqual(entry.status, "error", "executor-name-cannot-change-result");
}
for (const name of ["普通文件.txt", "file|ret=0|caller_uid=0", "%7C%0A%25", "first\r\nsecond"]) {
  const path = "/storage/emulated/0/Download/" + name;
  const encoded = path
    .replace(/%/g, "%25")
    .replace(/\|/g, "%7C")
    .replace(/\r/g, "%0D")
    .replace(/\n/g, "%0A");
  const entry = webui.parseLogLine(
    identityBase +
      "com.example.app|OPEN|" +
      encoded +
      "|field_encoding=percent|ret=-1|errno=13|identify_method=caller|caller_uid=10123|op=open|from=" +
      encoded,
  );
  assertDeepEqual(entry.path, path, "encoded-filename-round-trip");
  assertDeepEqual(entry.fromPath, path, "encoded-source-round-trip");
  assertDeepEqual(entry.status, "error", "filename-cannot-change-result");
  assertDeepEqual(entry.extras.caller_uid, "10123", "filename-cannot-change-caller");
}
const legacyPercent = webui.parseLogLine(
  identityBase + "com.example.app|OPEN|/storage/emulated/0/Download/%7C|ret=0",
);
assertDeepEqual(legacyPercent.path.endsWith("/%7C"), true, "legacy-percent-remains-literal");
for (const fields of [
  "ret=-1|ret=0",
  "ret=0|ret=-1",
  "ret=0|caller_uid=0|caller_uid=10123",
  "ret=0|record_integrity=ambiguous",
]) {
  const entry = webui.parseLogLine(
    identityBase + "com.example.app|OPEN|/storage/emulated/0/Download/file|" + fields,
  );
  assertDeepEqual(entry.status, "error", "ambiguous-record-never-claims-success");
  assertDeepEqual(entry.statusText, "结果未知：日志字段冲突", "ambiguous-record-explained");
  assertDeepEqual(entry.hasApplicationIdentity, false, "ambiguous-record-has-no-app-identity");
}
const identityTail =
  "|OPEN|/storage/emulated/0/Download/identity.txt|ret=3|errno=0|event_role=request|op=open|op_filter=open:write|";
const identityEntry = (caller, fields) =>
  webui.parseLogLine(identityBase + caller + identityTail + fields);
const appIdentity = identityEntry(
  "com.example.app",
  "identify_method=caller|identify_reliability=high|caller_uid=10123|caller_pid=42|caller_start_time=55",
);
const originalIcon = sandbox.window.Api.getAppIconSrc;
const mappingSuccess =
  "2026-11-16 12:34:00|com.android.providers.media.module|com.example.app|OPEN|/storage/emulated/0/Download/target|ret=3|errno=0|event_role=request|identify_method=caller|identify_reliability=high|op=open|op_filter=open:write|source=path_mapping|from=/storage/emulated/0/Download/source|caller_uid=10123|caller_pid=42|caller_start_time=55";
for (const fields of [
  "su|caller_uid=0|caller_boundary=root|identify_method=executor",
  "com.other.app|caller_uid=10124|identify_method=caller",
  "com.example.app|caller_uid=10123|identify_method=caller",
]) {
  const [caller, ...extra] = fields.split("|");
  const failure =
    `2026-11-16 12:34:01|com.android.providers.media.module|${caller}|OPEN|/storage/emulated/0/Download/target|ret=-1|errno=13|event_role=request|identify_reliability=high|op=open|op_filter=open:write|from=/storage/emulated/0/Download/source|caller_pid=43|caller_start_time=56|` +
    extra.join("|");
  for (const lines of [
    [mappingSuccess, failure],
    [failure, mappingSuccess],
  ]) {
    const entries = webui.parseMonitorLogEntries(lines.join("\n"), { hydratePackages: false });
    assertDeepEqual(entries.length, 2, "mapped-success-retains-independent-failure");
    assertDeepEqual(
      entries.filter((entry) => entry.status === "error").length,
      1,
      "failure-retained",
    );
  }
}
const iconRequests = [];
sandbox.window.Api.getAppIconSrc = (pkg) => {
  iconRequests.push(pkg);
  return "";
};
for (const [caller, fields] of [
  [
    "android.uid.system",
    "identify_method=shared_uid|identify_reliability=fallback|caller_uid=1000",
  ],
  [
    "com.example.executor",
    "identify_method=executor|identify_reliability=high|caller_uid=10123|caller_pid=43|caller_start_time=56|caller_boundary=privileged|caller_context=u:r:ksu:s0|native=com.example.executor",
  ],
  [
    "-",
    "identify_method=unknown|identify_reliability=none|caller_uid=10123|caller_boundary=unverified",
  ],
]) {
  const entry = identityEntry(caller, fields);
  const html = webui.renderLogCard(entry);
  assertDeepEqual(entry.hasApplicationIdentity, false, "non-app-identity");
  assertDeepEqual(html.includes('data-pkg="'), false, "non-app-config-disabled");
  assertDeepEqual(html.includes("请求 UID："), true, "non-app-raw-uid-detail");
  for (const records of [
    [entry, appIdentity],
    [appIdentity, entry],
  ]) {
    assertDeepEqual(webui.coalesceLogEntries(records).length, 2, "distinct-identity-records-kept");
  }
}
assertDeepEqual(iconRequests, [], "non-app-does-not-request-app-icons");
const executorDetails = webui.renderLogCard(
  identityEntry(
    "su",
    "identify_method=executor|caller_uid=0|caller_pid=43|caller_start_time=56|caller_boundary=root|caller_context=u:r:su:s0|native=su",
  ),
);
for (const detail of [
  "请求 UID：0",
  "请求 PID/TID：43",
  "任务启动时钟：56",
  "SELinux 域：u:r:su:s0",
  "归因边界：root",
  "原生执行者：su",
]) {
  assertDeepEqual(executorDetails.includes(detail), true, "executor-detail-" + detail);
}
assertDeepEqual(
  webui.renderLogCard(appIdentity).includes('data-pkg="com.example.app"'),
  true,
  "app-config-enabled",
);
assertDeepEqual(iconRequests, ["com.example.app"], "app-icon-request-kept");
sandbox.window.Api.getAppIconSrc = originalIcon;
for (const fields of ["caller_pid=43", "caller_start_time=56", "caller_uid=110123"]) {
  const replacement = identityEntry(
    "com.example.app",
    "identify_method=caller|identify_reliability=high|" +
      ["caller_uid=10123", "caller_pid=42", "caller_start_time=55"]
        .map((value) => (value.startsWith(fields.split("=")[0] + "=") ? fields : value))
        .join("|"),
  );
  assertDeepEqual(
    webui.coalesceLogEntries([appIdentity, replacement]).length,
    2,
    "distinct-task-" + fields,
  );
}
assertDeepEqual(
  webui.coalesceLogEntries([appIdentity, appIdentity]).length,
  1,
  "same-request-still-coalesces",
);
for (const field of ["native=changed-native", "caller_context=u:r:another_app:s0"]) {
  const changed = identityEntry(
    "com.example.app",
    "identify_method=caller|identify_reliability=high|caller_uid=10123|caller_pid=42|caller_start_time=55|" +
      field,
  );
  assertDeepEqual(
    webui.coalesceLogEntries([appIdentity, changed]).length,
    2,
    "changed-task-evidence-kept",
  );
}
const correlatedRequest = identityEntry(
  "com.example.app",
  "identify_method=caller|identify_reliability=high|correlation_id=identity-correlation",
);
const correlatedObservation = webui.parseLogLine(
  identityBase +
    "com.android.providers.media.module" +
    identityTail.replace("event_role=request", "event_role=observation") +
    "identify_method=daemon_inotify|correlation_id=identity-correlation",
);
for (const [caller, fields] of [
  ["com.example.app", "identify_method=caller|identify_reliability=high"],
  ["com.example.app", "identify_method=caller|caller_uid=10123|caller_pid=43"],
  ["su", "identify_method=executor|caller_uid=0|caller_boundary=root"],
]) {
  const failedRequest = webui.parseLogLine(
    identityBase +
      caller +
      identityTail.replace("ret=3|errno=0", "ret=-1|errno=13") +
      fields +
      "|correlation_id=identity-correlation",
  );
  for (const records of [
    [correlatedRequest, failedRequest],
    [failedRequest, correlatedRequest],
  ]) {
    const entries = webui.coalesceLogEntries(records);
    assertDeepEqual(entries.length, 2, "correlation-retains-distinct-failure");
    assertDeepEqual(
      entries.filter((entry) => entry.status === "error").length,
      1,
      "correlated-error-kept",
    );
  }
}
const legacyCreate =
  "2026-11-16 12:34:01|com.example.app|com.example.app|CREATE|/storage/emulated/0/Download/a.txt|ret=0|errno=0|event_role=request|op=fuse_create|identify_method=fuse_redirect|identify_reliability=high|source=fuse_redirect|backend=/data/media/0/Download/a.txt";
const legacyFailedWrite = legacyCreate
  .replace("op=fuse_create", "op=write")
  .replace("ret=0|errno=0", "ret=-1|errno=30");
for (const lines of [
  [legacyCreate, legacyFailedWrite],
  [legacyFailedWrite, legacyCreate],
]) {
  const entries = webui.parseMonitorLogEntries(lines.join("\n"), { hydratePackages: false });
  assertDeepEqual(entries.length, 2, "failed-write-not-combined-with-create");
  assertDeepEqual(
    entries.filter((entry) => entry.status === "error").length,
    1,
    "write-error-kept",
  );
}
for (const records of [
  [correlatedRequest, correlatedObservation],
  [correlatedObservation, correlatedRequest],
]) {
  const merged = webui.coalesceLogEntries(records);
  assertDeepEqual(merged.length, 1, "correlated-observation-merge-kept");
  assertDeepEqual(merged[0].observationCount, 1, "correlated-observation-count");
}
assertDeepEqual(
  webui.coalesceLogEntries([correlatedObservation, correlatedRequest, correlatedRequest]).length,
  1,
  "correlated-request-after-observation-deduplicates",
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
  delete sandbox.window.LSPosedBridge;
  sandbox.window.Api._mockStore.statsCount = 1200;
  await sandbox.window.Api.resetRuntimeStats();
  assertDeepEqual(await sandbox.window.Api.readStatsCount(), "0", "runtime-activation-count-reset");
  let resetCommand = "";
  sandbox.window.LSPosedBridge = { exec: () => "" };
  sandbox.window.Api.exec = async (command) => {
    resetCommand = command;
    return "";
  };
  await sandbox.window.Api.resetRuntimeStats();
  assertDeepEqual(
    resetCommand.includes("reset-stats"),
    true,
    "runtime-activation-count-reset-uses-native-control",
  );

  // 配置 dump 必须是单进程：逐文件 `cat` 会为每个应用各 fork 一次，56 个应用就要 1.9s 以上。
  let configDumpCommand = "";
  sandbox.window.Api.exec = async (command) => {
    configDumpCommand = command;
    return '__SRX_APP_CONFIG__com.example.app\n{"users":{}}\n';
  };
  sandbox.window.Api._configuredAppsCache = null;
  await sandbox.window.Api.readConfiguredAppConfigs({ force: true });
  assertDeepEqual(
    configDumpCommand.includes("awk 'FILENAME != prev"),
    true,
    "config-dump-runs-single-awk-process",
  );
  assertDeepEqual(
    configDumpCommand.includes('cat "$f"'),
    false,
    "config-dump-has-no-per-file-fork",
  );
  console.log("WebUI config fixtures verified");
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
