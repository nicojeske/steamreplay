// Downloads the pinned laughter model (EfficientAT mn10_as as ONNX, M15) that the app bundles as a
// resource. Run with `pnpm fetch-model` (Node strips the types). CI runs it before every build.
//
// The model is an asset of this repo's release `model-mn10as-v1` (made by
// experiments/moments/export_onnx.py). The repo is private, so the download goes through the GitHub
// API with a token: GH_TOKEN or GITHUB_TOKEN, else `gh auth token`. A new model needs a new release
// tag and new constants here and in `MODEL_SHA256` (src-tauri/src/media/laughter/mod.rs).

import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const REPO = "nicojeske/steamreplay";
const TAG = "model-mn10as-v1";
const ASSET = "mn10_as.onnx";
const SHA256 = "b59b560b0d64f5a094bb5b2b40fb20a65eeb4c9a3e5cd8b183c66f4265d0f978";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const modelDir = join(root, "src-tauri", "resources", "models");
const output = join(modelDir, ASSET);

function sha256(data: Buffer): string {
  return createHash("sha256").update(data).digest("hex");
}

function token(): string {
  const env = process.env.GH_TOKEN || process.env.GITHUB_TOKEN;
  if (env) return env;
  try {
    return execFileSync("gh", ["auth", "token"], { encoding: "utf8" }).trim();
  } catch {
    throw new Error("no GitHub token: set GH_TOKEN or log in with `gh auth login` (the repo is private)");
  }
}

async function main() {
  if (existsSync(output) && sha256(readFileSync(output)) === SHA256) {
    console.log(`${ASSET} already in place`);
    return;
  }
  const headers = { Authorization: `Bearer ${token()}`, "X-GitHub-Api-Version": "2022-11-28" };
  const release = await fetch(`https://api.github.com/repos/${REPO}/releases/tags/${TAG}`, { headers });
  if (!release.ok) throw new Error(`release ${TAG}: HTTP ${release.status} ${release.statusText}`);
  const { assets } = (await release.json()) as { assets: { name: string; url: string }[] };
  const asset = assets.find((a) => a.name === ASSET);
  if (!asset) throw new Error(`release ${TAG} has no ${ASSET}`);

  console.log(`downloading ${ASSET} from ${REPO}@${TAG}`);
  const res = await fetch(asset.url, { headers: { ...headers, Accept: "application/octet-stream" } });
  if (!res.ok) throw new Error(`download failed: HTTP ${res.status} ${res.statusText}`);
  const data = Buffer.from(await res.arrayBuffer());
  const hash = sha256(data);
  if (hash !== SHA256) throw new Error(`checksum mismatch: expected ${SHA256}, got ${hash}`);
  mkdirSync(modelDir, { recursive: true });
  writeFileSync(`${output}.tmp`, data);
  renameSync(`${output}.tmp`, output);
  console.log(`${ASSET} → ${modelDir}`);
}

main().catch((e: unknown) => {
  console.error(e instanceof Error ? e.message : e);
  process.exit(1);
});
