/**
 * 校验 WebUI 对 system_server PackageManager 应用清单的解析及用户隔离。
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

vm.runInNewContext(fs.readFileSync(apiJsPath, "utf8"), sandbox, {
  filename: apiJsPath,
});

function assertEqual(actual, expected, label) {
  if (actual !== expected) {
    throw new Error(`${label}: expected ${String(expected)}, got ${String(actual)}`);
  }
}

const parseUsers = sandbox.parsePackageUsers;
const users = parseUsers("0\n10\n10\ninvalid\n");
assertEqual(users.length, 2, "user-id-count");
assertEqual(users[1], "10", "secondary-user-id");

const parse = sandbox.parseSystemServerAppList;
if (typeof parse !== "function") throw new Error("parseSystemServerAppList is not defined");

const ownerApps = parse(
  [
    "0\t10123\t0\tcom.example.owner\tOwner%20App\t%2Fdata%2Fapp%2Fowner%2Fbase.apk",
    "0\t1000\t1\tcom.android.settings\tSettings%20%E2%9C%93\t%2Fsystem%2Fpriv-app%2FSettings%2Fbase.apk",
  ].join("\n"),
  "0",
);
assertEqual(ownerApps.length, 2, "owner-app-count");
assertEqual(ownerApps[0].userId, "0", "owner-user-id");
assertEqual(ownerApps[0].appLabel, "Owner App", "owner-app-label");
assertEqual(ownerApps[1].isSystem, true, "system-app-marker");
assertEqual(ownerApps[1].appLabel, "Settings ✓", "unicode-label");

const workApps = parse(
  "10\t1012345\t0\tcom.example.work\tWork%09App\t%2Fdata%2Fapp%2Fwork%2Fbase.apk\n",
  "10",
);
assertEqual(workApps.length, 1, "work-app-count");
assertEqual(workApps[0].packageName, "com.example.work", "work-app-package");
assertEqual(workApps[0].appLabel, "Work\tApp", "escaped-tab-label");
assertEqual(
  parse("0\t10123\t0\tcom.example.owner\tOwner\t%2Fdata%2Fapp.apk", "10"),
  null,
  "cross-user-row-rejected",
);
assertEqual(
  parse("10\t10123\t0\tcom.example.work\tBad%2\t%2Fdata%2Fapp.apk", "10"),
  null,
  "malformed-label-rejected",
);
assertEqual(
  parse("10\t2147483648\t0\tcom.example.work\tWork\t%2Fdata%2Fapp.apk", "10"),
  null,
  "overflow-uid-rejected",
);
assertEqual(
  parse("10\t10123\t0\tcom.example.work\tWork\t%2Fdata%2Fapp.apk", "10"),
  null,
  "cross-user-uid-rejected",
);
assertEqual(
  parse("10\t10123\t0\tcom.example.work\tWork\trelative.apk", "10"),
  null,
  "relative-apk-path-rejected",
);

async function verifyInstalledAppsUsePerUserPackageManagerResults() {
  const api = sandbox.window.Api;
  const usersFromPackageManager = await api.listUsers();
  const ownerApps = await api.getInstalledApps("0");
  const workApps = await api.getInstalledApps("10");
  assertEqual(usersFromPackageManager.join(","), "0,10", "api-user-list");
  assertEqual(ownerApps.includes("com.example.work"), false, "owner-does-not-see-work-only-app");
  assertEqual(workApps.includes("com.example.work"), true, "work-user-sees-work-only-app");
  assertEqual(
    api.getCachedAppInfo("com.example.work", "10").appLabel,
    "Work App",
    "work-user-label-cache",
  );
  assertEqual(
    api.getCachedAppInfo("com.android.settings", "0").isSystem,
    true,
    "owner-system-app-cache",
  );
}

verifyInstalledAppsUsePerUserPackageManagerResults()
  .then(() => console.log("WebUI PackageManager app inventory verified"))
  .catch((error) => {
    console.error(error);
    process.exitCode = 1;
  });
