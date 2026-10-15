// Downloads the pinned ffmpeg build that the Windows installer bundles as a Tauri sidecar.
// Run with `pnpm fetch-ffmpeg` (Node strips the types). CI runs it before building on Windows.
//
// Pinned: BtbN GPL static build. BtbN keeps the monthly (end-of-month) autobuilds; if this URL
// ever 404s, pick a newer `ffmpeg-n7.1*-win64-gpl-7.1.zip` and update all three constants.

import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const RELEASE = "autobuild-2026-07-31-14-10";
const ASSET = "ffmpeg-n7.1.5-12-g1fdbca85aa-win64-gpl-7.1";
const SHA256 = "c067a1ca58f4fc4449f4bab0890fbcd65cbb3e5f46e066cf9c768e06c0c1d4d9";

const URL = `https://github.com/BtbN/FFmpeg-Builds/releases/download/${RELEASE}/${ASSET}.zip`;
const TRIPLE = "x86_64-pc-windows-msvc";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const binDir = join(root, "src-tauri", "binaries");
const licenseDir = join(root, "src-tauri", "resources", "ffmpeg");
const stamp = join(binDir, ".ffmpeg-build");
const outputs = {
  ffmpeg: join(binDir, `ffmpeg-${TRIPLE}.exe`),
  ffprobe: join(binDir, `ffprobe-${TRIPLE}.exe`),
  license: join(licenseDir, "LICENSE.txt"),
};

function upToDate(): boolean {
  return (
    Object.values(outputs).every((p) => existsSync(p)) &&
    existsSync(stamp) &&
    readFileSync(stamp, "utf8").trim() === ASSET
  );
}

async function main() {
  if (upToDate()) {
    console.log(`ffmpeg ${ASSET} already in place`);
    return;
  }
  console.log(`downloading ${URL}`);
  const res = await fetch(URL);
  if (!res.ok) throw new Error(`download failed: HTTP ${res.status} ${res.statusText}`);
  const zip = Buffer.from(await res.arrayBuffer());
  const hash = createHash("sha256").update(zip).digest("hex");
  if (hash !== SHA256) throw new Error(`checksum mismatch: expected ${SHA256}, got ${hash}`);

  const work = mkdtempSync(join(tmpdir(), "steamreplay-ffmpeg-"));
  try {
    const zipPath = join(work, `${ASSET}.zip`);
    writeFileSync(zipPath, zip);
    // Windows' tar is bsdtar and reads zip files; GNU tar does not.
    if (process.platform === "win32") execFileSync("tar", ["-xf", zipPath, "-C", work], { stdio: "inherit" });
    else execFileSync("unzip", ["-q", zipPath, "-d", work], { stdio: "inherit" });
    const extracted = join(work, ASSET);
    mkdirSync(binDir, { recursive: true });
    mkdirSync(licenseDir, { recursive: true });
    copyFileSync(join(extracted, "bin", "ffmpeg.exe"), outputs.ffmpeg);
    copyFileSync(join(extracted, "bin", "ffprobe.exe"), outputs.ffprobe);
    copyFileSync(join(extracted, "LICENSE.txt"), outputs.license);
    writeFileSync(stamp, `${ASSET}\n`);
    console.log(`ffmpeg ${ASSET} → ${binDir}`);
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

main().catch((e: unknown) => {
  console.error(e instanceof Error ? e.message : e);
  process.exit(1);
});
