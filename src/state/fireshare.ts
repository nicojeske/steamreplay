import { create } from "zustand";
import { commands, type FireshareConfig, type FireshareOptions, type Game, type UploadMeta } from "../bindings";
import { unwrap } from "../lib/ipc";

/** What the upload dialog is opened for. */
export type UploadRequest = {
  /** Steam's game name: preselects the Fireshare game. */
  gameName: string;
  defaultTitle: string;
  /** "Upload" or "Export & upload". */
  submitLabel: string;
  onSubmit: (meta: UploadMeta) => void;
};

type FireshareState = {
  /** `null` until loaded. */
  config: FireshareConfig | null;
  /** Games and tags of the server, cached for the session. */
  options: FireshareOptions | null;
  request: UploadRequest | null;
  /** Bumped when the user wants the settings opened at the Fireshare section. */
  settingsSeq: number;
  loadConfig: () => Promise<FireshareConfig>;
  setConfig: (config: FireshareConfig) => void;
  loadOptions: (force?: boolean) => Promise<FireshareOptions>;
  openUpload: (request: UploadRequest) => void;
  closeUpload: () => void;
  openSettings: () => void;
};

export const isConfigured = (c: FireshareConfig | null) => !!c?.url && c.hasToken;

export const useFireshare = create<FireshareState>((set, get) => ({
  config: null,
  options: null,
  request: null,
  settingsSeq: 0,
  loadConfig: async () => {
    const config = await unwrap(commands.getFireshareConfig());
    set({ config });
    return config;
  },
  // A new server or token may offer other games and tags.
  setConfig: (config) => set({ config, options: null }),
  loadOptions: async (force) => {
    const cached = get().options;
    if (cached && !force) return cached;
    const options = await unwrap(commands.fireshareOptions());
    set({ options });
    return options;
  },
  openUpload: (request) => set({ request }),
  closeUpload: () => set({ request: null }),
  openSettings: () => set((s) => ({ request: null, settingsSeq: s.settingsSeq + 1 })),
}));

/** `Deadlock – 17 Oct 2026, 21:03`. */
export function defaultTitle(gameName: string, at: Date): string {
  if (Number.isNaN(at.getTime())) return gameName;
  return `${gameName} – ${at.toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" })}`;
}

const simplify = (name: string) => name.toLowerCase().replace(/[^\p{L}\p{N}]+/gu, "");

/** The Fireshare game for a Steam game: the last choice, else one with the same name. */
export function guessGame(games: Game[], steamGame: string, remembered: number | null): number | null {
  if (remembered !== null && games.some((g) => g.id === remembered)) return remembered;
  const key = simplify(steamGame);
  return games.find((g) => simplify(g.name) === key)?.id ?? null;
}
