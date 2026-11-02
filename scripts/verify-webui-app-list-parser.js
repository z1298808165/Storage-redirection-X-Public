/**
 * 校验 WebUI 对 `tools/list_apps/Main.java` 输出格式的解析。
 *
 * 这里锁定的是跨版本契约：
 * - 旧 dex（无 `#system=` 行）必须仍能解析出包名与标签，且不臆造系统标记；
 * - 新 dex 的 `#system=` 行必须被识别，且不会污染标签映射或被当成应用条目。
 */
const fs = require("fs");
const path = require("path");
const vm = require("vm");

const repoRoot = path.resolve(__dirname, "..");
const apiJsPath = path.join(repoRoot, "assets/zygisk_module/webroot/js/api.js");

const sandbox = {
  console,
  setTimeout: () => {},
  clearTimeout: () => {},
  localStorage: { getItem: () => null, setItem: () => {} },
  window: {},
};
sandbox.window.window = sandbox.window;
sandbox.globalThis = sandbox;

vm.runInNewContext(fs.readFileSync(apiJsPath, "utf8"), sandbox, { filename: apiJsPath });

function assertEqual(actual, expected, label) {
  if (actual !== expected) {
    throw new Error(`${label}: expected ${String(expected)}, got ${String(actual)}`);
  }
}

const parse = sandbox.parseCompatAppList;
if (typeof parse !== "function") throw new Error("parseCompatAppList is not defined");

const withSystemMarker = parse(
  [
    "com.example.alpha=Alpha",
    "com.android.settings=设置",
    "bad/pkg=Bad",
    "# 旧实现产生的注释行",
    "#system=com.android.settings bad/pkg",
    "",
  ].join("\n"),
);
assertEqual(withSystemMarker.length, 2, "system-marker-app-count");
assertEqual(withSystemMarker[0].packageName, "com.example.alpha", "system-marker-first-package");
assertEqual(withSystemMarker[0].isSystem, false, "system-marker-user-app");
assertEqual(withSystemMarker[1].isSystem, true, "system-marker-system-app");

const withoutSystemMarker = parse("com.example.alpha=Alpha\n");
assertEqual(withoutSystemMarker.length, 1, "legacy-app-count");
assertEqual(withoutSystemMarker[0].appLabel, "Alpha", "legacy-app-label");
assertEqual("isSystem" in withoutSystemMarker[0], false, "legacy-app-keeps-fallback");

const markerOnly = parse("#system=com.android.settings\n");
assertEqual(markerOnly.length, 0, "marker-only-app-count");

const bareNames = parse("org.demo.beta Beta Label\n");
assertEqual(bareNames.length, 1, "bare-name-app-count");
assertEqual(bareNames[0].packageName, "org.demo.beta", "bare-name-package");

// `pm list packages -f -U` + `-s` 两段输出：成员带 APK 路径（可能含 `==`），系统集合只取 `-s` 段。
const listPackages = sandbox.parseUserPackageListing;
if (typeof listPackages !== "function") throw new Error("parseUserPackageListing is not defined");

const listing = listPackages(
  [
    "package:/data/app/alpha/base.apk=com.example.alpha uid:10001",
    "package:/data/app/~~MPMDBgih6vWQNh_eSekUiA==/com.google.android.webview-6SH5==/base.apk=com.google.android.webview uid:99910173",
    "package:/system/app/beta/base.apk=bad/pkg uid:10003",
    "srx-system-section",
    "package:com.google.android.webview",
    "package:bad/pkg",
    "",
  ].join("\n"),
  "srx-system-section",
);
assertEqual(listing.packages.length, 2, "listing-member-count");
assertEqual(listing.packages[1], "com.google.android.webview", "listing-apk-path-with-equals");
assertEqual(listing.system.size, 1, "listing-system-count");
assertEqual(listing.system.has("com.google.android.webview"), true, "listing-system-membership");
assertEqual(listing.system.has("com.example.alpha"), false, "listing-user-app-not-system");

// 标记缺失时不能把 `-s` 段混进成员集合（旧实现被 sh 注释吃掉就是这样退化的）。
const noMarker = listPackages(
  "package:/data/app/alpha/base.apk=com.example.alpha\n",
  "srx-system-section",
);
assertEqual(noMarker.packages.length, 1, "listing-without-marker-members");
assertEqual(noMarker.system.size, 0, "listing-without-marker-system");

console.log("WebUI app list parser verified");
