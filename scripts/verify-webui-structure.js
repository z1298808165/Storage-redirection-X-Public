const fs = require("fs");
const path = require("path");

const repoRoot = path.resolve(__dirname, "..");
const files = [
  "assets/zygisk_module/webroot/js/api.js",
  "assets/zygisk_module/webroot/js/app.js",
  "assets/zygisk_module/webroot/js/theme.js",
];

function duplicates(entries) {
  const linesByName = new Map();
  for (const entry of entries) {
    const lines = linesByName.get(entry.name) || [];
    lines.push(entry.line);
    linesByName.set(entry.name, lines);
  }
  return [...linesByName.entries()].filter(([, lines]) => lines.length > 1);
}

function matches(lines, pattern) {
  const entries = [];
  lines.forEach((line, index) => {
    const match = pattern.exec(line);
    if (match) entries.push({ name: match[1], line: index + 1 });
  });
  return entries;
}

const errors = [];
for (const relativePath of files) {
  const lines = fs.readFileSync(path.join(repoRoot, relativePath), "utf8").split(/\r?\n/);
  const functions = matches(lines, /^(?:async\s+)?function\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*\(/);
  for (const [name, lineNumbers] of duplicates(functions)) {
    errors.push(`${relativePath}: duplicate function ${name} at lines ${lineNumbers.join(", ")}`);
  }

  if (relativePath.endsWith("/api.js")) {
    const apiStart = lines.findIndex((line) => line === "const Api = {");
    if (apiStart < 0) {
      errors.push(`${relativePath}: Api object not found`);
      continue;
    }
    const apiEnd = lines.findIndex((line, index) => index > apiStart && line === "};");
    if (apiEnd < 0) {
      errors.push(`${relativePath}: Api object end not found`);
      continue;
    }
    const methods = matches(
      lines.slice(apiStart + 1, apiEnd),
      /^  (?:async\s+)?([A-Za-z_$][A-Za-z0-9_$]*)\s*\([^)]*\)\s*\{/,
    ).map((entry) => ({ ...entry, line: entry.line + apiStart + 1 }));
    for (const [name, lineNumbers] of duplicates(methods)) {
      errors.push(
        `${relativePath}: duplicate Api method ${name} at lines ${lineNumbers.join(", ")}`,
      );
    }
  }
}

if (errors.length) throw new Error(errors.join("\n"));
console.log("WebUI structure verified");
