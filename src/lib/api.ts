import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Loader = "vanilla" | "fabric" | "quilt" | "forge" | "neoforge";
export type VersionType = "release" | "snapshot" | "old_beta" | "old_alpha";
export type Phase =
  | "idle"
  | "auth"
  | "resolve"
  | "loader"
  | "java"
  | "download"
  | "kzengine"
  | "launching"
  | "running";

export interface Account {
  uuid: string;
  username: string;
  skinUrl: string | null;
  skinSlim: boolean;
  capeUrl: string | null;
}
export interface AccountsView {
  accounts: Account[];
  active: string | null;
}
export interface VersionEntry {
  id: string;
  type: VersionType;
  releaseTime: string;
}
export interface SystemInfo {
  totalMb: number;
  recommendedMb: number;
  maxMb: number;
}
export interface LaunchOptions {
  version: string;
  loader: Loader;
  loaderVersion: string | null;
  kzEngine: boolean;
  ramMb: number;
}
export interface ProgressEvent {
  stage: string;
  filesDone: number;
  filesTotal: number;
  bytesDone: number;
  bytesTotal: number;
  speed: number;
}
export interface PhaseEvent {
  phase: Phase;
  detail: string;
}
export interface LogLine {
  stream: "stdout" | "stderr";
  level: string | null;
  line: string;
}
export interface ExitEvent {
  code: number | null;
}
export interface AuthWarning {
  uuid: string;
  message: string;
}

/** Texture Steve officielle (utilisée sans compte). */
export const STEVE_SKIN =
  "https://textures.minecraft.net/texture/31f477eb1a7beee631c2ca64d06f8f68fa93a3386d04452ab27f43acdf1b60cb";

export const api = {
  systemInfo: () => invoke<SystemInfo>("system_info"),
  listVersions: () => invoke<VersionEntry[]>("list_versions"),
  login: () => invoke<Account>("auth_login"),
  accounts: () => invoke<AccountsView>("auth_accounts"),
  switchAccount: (uuid: string) => invoke<Account>("auth_switch", { uuid }),
  removeAccount: (uuid: string) => invoke<AccountsView>("auth_remove", { uuid }),
  fetchSkin: (url: string) => invoke<string>("fetch_skin", { url }),
  launch: (opts: LaunchOptions) => invoke<void>("launch", { opts }),
};

/** Abonnement à un évènement backend, sûr même si le composant se démonte avant la résolution. */
export function on<T>(event: string, cb: (payload: T) => void): () => void {
  let unlisten: UnlistenFn | null = null;
  let disposed = false;
  listen<T>(event, (e) => cb(e.payload))
    .then((u) => {
      if (disposed) u();
      else unlisten = u;
    })
    .catch(() => undefined);
  return () => {
    disposed = true;
    unlisten?.();
  };
}

export function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  try {
    return JSON.stringify(e);
  } catch {
    return "Erreur inconnue";
  }
}

const skinCache = new Map<string, Promise<string>>();

/** Skin en data URL via le proxy backend (mis en cache, nouvel essai possible après un échec). */
export function skinData(url: string | null): Promise<string> {
  const key = url ?? STEVE_SKIN;
  let p = skinCache.get(key);
  if (!p) {
    p = api.fetchSkin(key);
    p.catch(() => skinCache.delete(key));
    skinCache.set(key, p);
  }
  return p;
}

/** Tête 2D (couche de base + couche chapeau) extraite d'une texture de skin. */
export function headFromSkin(dataUrl: string, size = 64): Promise<string> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => {
      const canvas = document.createElement("canvas");
      canvas.width = size;
      canvas.height = size;
      const ctx = canvas.getContext("2d");
      if (!ctx) return reject(new Error("Canvas 2D indisponible"));
      ctx.imageSmoothingEnabled = false;
      ctx.drawImage(img, 8, 8, 8, 8, 0, 0, size, size);
      ctx.drawImage(img, 40, 8, 8, 8, 0, 0, size, size);
      resolve(canvas.toDataURL());
    };
    img.onerror = () => reject(new Error("Skin illisible"));
    img.src = dataUrl;
  });
}

export const formatGb = (mb: number) => `${(mb / 1024).toFixed(mb % 1024 === 0 ? 0 : 1)} Go`;
