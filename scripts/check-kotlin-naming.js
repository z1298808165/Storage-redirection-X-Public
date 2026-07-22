const fs = require("fs");
const path = require("path");

const root = path.resolve(__dirname, "..");
const sourceRoots = ["app/src", "tests/storage-redirect-test"];
const ignoredDirs = new Set(["build", ".gradle", ".kotlin"]);
const packageWithUnderscore = /^\s*package\s+\S*_\S*/gm;
const prefixedInterface = /\binterface\s+I[A-Z][A-Za-z0-9_]*\b/g;
const longDeclaration = /\b(class|object|interface|fun|val|var)\s+([A-Za-z0-9_]{51,})\b/g;
const violations = [];

function visit(directory) {
  if (!fs.existsSync(directory)) return;
  for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
    if (entry.isDirectory() && ignoredDirs.has(entry.name)) continue;
    const target = path.join(directory, entry.name);
    if (entry.isDirectory()) visit(target);
    else if (entry.isFile() && entry.name.endsWith(".kt")) checkFile(target);
  }
}

function checkFile(file) {
  const text = fs.readFileSync(file, "utf8");
  const relative = path.relative(root, file).replace(/\\/g, "/");
  for (const match of text.matchAll(packageWithUnderscore)) {
    violations.push(`${relative}: package names must not contain underscores (${match[0].trim()})`);
  }
  for (const match of text.matchAll(prefixedInterface)) {
    violations.push(`${relative}: avoid I-prefixed interfaces (${match[0]})`);
  }
  for (const match of text.matchAll(longDeclaration)) {
    violations.push(`${relative}: declaration name is too long (${match[2]})`);
  }
}

sourceRoots.forEach((sourceRoot) => visit(path.join(root, sourceRoot)));
if (violations.length > 0) {
  console.error(violations.join("\n"));
  process.exit(1);
}
console.log("Kotlin naming verified");
