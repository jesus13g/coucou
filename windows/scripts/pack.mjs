// Copies the installer Tauri buries in target/release/bundle/nsis/ into
// windows/release/, with the name it ships under. Used by `npm run pack` and by
// the release workflow, so both produce exactly the same file names.
// On Linux it collects the .deb, .rpm and .AppImage bundles instead.

import { readFileSync, mkdirSync, copyFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const bundleDir = join(root, "target", "release", "bundle", "nsis");
const outDir = join(root, "release");

const { version } = JSON.parse(readFileSync(join(root, "src-tauri", "tauri.conf.json"), "utf8"));

if (process.platform === "linux") {
  packLinux();
  process.exit(0);
}

/** Newest file in `dir` whose name ends with `suffix`, or null. */
function newest(dir, suffix) {
  let files = [];
  try {
    files = readdirSync(dir).filter((f) => f.endsWith(suffix));
  } catch {
    return null;
  }
  if (files.length === 0) return null;
  return files
    .map((f) => join(dir, f))
    .sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs)[0];
}

function packLinux() {
  const bundles = join(root, "target", "release", "bundle");
  const arch = process.arch === "arm64" ? "aarch64" : "x86_64";
  const wanted = [
    ["deb", ".deb"],
    ["rpm", ".rpm"],
    ["appimage", ".AppImage"],
  ];
  mkdirSync(outDir, { recursive: true });
  const shipped = [];
  for (const [folder, ext] of wanted) {
    const built = newest(join(bundles, folder), ext);
    if (!built) continue;
    const dest = join(outDir, `Coucou-Linux-${version}-${arch}${ext}`);
    copyFileSync(built, dest);
    shipped.push(dest);
  }
  if (shipped.length === 0) {
    console.error(`No bundle in ${bundles} — run \`npm run tauri build\` first.`);
    process.exit(1);
  }
  console.log("\n  Linux bundles ready\n");
  for (const f of shipped) console.log(`  ${f}  (${(statSync(f).size / 1024 / 1024).toFixed(2)} MB)`);
  console.log("");
}

let installers = [];
try {
  installers = readdirSync(bundleDir).filter((f) => f.endsWith("-setup.exe"));
} catch {
  console.error(`No installer in ${bundleDir} — run \`npm run tauri build\` first.`);
  process.exit(1);
}
if (installers.length === 0) {
  console.error(`No installer in ${bundleDir} — run \`npm run tauri build\` first.`);
  process.exit(1);
}

// Newest wins, in case an older build is still lying around.
const built = installers
  .map((f) => join(bundleDir, f))
  .sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs)[0];

mkdirSync(outDir, { recursive: true });
const versioned = join(outDir, `Coucou-Windows-${version}-setup.exe`);
const rolling = join(outDir, "Coucou-Windows-setup.exe");
copyFileSync(built, versioned);
copyFileSync(built, rolling);

const mb = (statSync(versioned).size / 1024 / 1024).toFixed(2);
console.log(`\n  Installer ready — ${mb} MB\n`);
console.log(`  ${versioned}`);
console.log(`  ${rolling}\n`);
