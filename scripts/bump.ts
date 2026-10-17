// Sets the app version, commits and tags it: `pnpm bump 0.2.0` (Node strips the types).
// `package.json` is the source of truth (`tauri.conf.json` points at it); `Cargo.toml` and
// `Cargo.lock` are kept in step. Does not push: `git push --follow-tags` starts the release.

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const SEMVER = /^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/;

function git(...args: string[]): string {
  return execFileSync("git", args, { cwd: root, encoding: "utf8" }).trim();
}

function main() {
  const version = process.argv[2];
  if (!version || !SEMVER.test(version)) throw new Error("usage: pnpm bump <x.y.z[-pre]>");
  if (git("status", "--porcelain") !== "") throw new Error("working tree is not clean");
  const tag = `v${version}`;
  if (git("tag", "--list", tag) !== "") throw new Error(`tag ${tag} already exists`);

  const pkgPath = join(root, "package.json");
  const pkg = readFileSync(pkgPath, "utf8");
  writeFileSync(pkgPath, pkg.replace(/("version":\s*")[^"]+(")/, `$1${version}$2`));

  // Only the `version` line of the `[package]` table, which is the first one in the file.
  const cargoPath = join(root, "src-tauri", "Cargo.toml");
  const cargo = readFileSync(cargoPath, "utf8");
  writeFileSync(cargoPath, cargo.replace(/^version = "[^"]+"$/m, `version = "${version}"`));

  execFileSync("cargo", ["update", "-p", "steamreplay", "--offline"], {
    cwd: join(root, "src-tauri"),
    stdio: "inherit",
  });

  git("add", "package.json", "src-tauri/Cargo.toml", "src-tauri/Cargo.lock");
  git("commit", "-m", `release: ${tag}`);
  git("tag", "-a", tag, "-m", `SteamReplay ${tag}`);
  console.log(`committed and tagged ${tag}; push with: git push --follow-tags`);
}

try {
  main();
} catch (e: unknown) {
  console.error(e instanceof Error ? e.message : e);
  process.exit(1);
}
