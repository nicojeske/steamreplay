import type { Backend } from "../bindings";

/** Names of the preview / decode backends for people. */
export const BACKEND_NAMES: Record<Backend, string> = {
  cuda: "NVIDIA (CUDA / NVENC)",
  d3d11vaAmf: "AMD (D3D11VA / AMF)",
  d3d11vaQsv: "Intel (D3D11VA / Quick Sync)",
  vaapi: "VAAPI (AMD / Intel on Linux)",
  qsv: "Intel Quick Sync",
  software: "Software (CPU)",
};
