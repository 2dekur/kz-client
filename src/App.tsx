import { useCallback, useEffect, useRef, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import {
  api,
  errorText,
  on,
  skinData,
  type AccountsView,
  type AuthWarning,
  type ExitEvent,
  type LaunchOptions,
  type LogLine,
  type PhaseEvent,
  type ProgressEvent,
  type SystemInfo,
  type VersionEntry,
} from "./lib/api";
import { AccountPanel } from "./components/AccountPanel";
import { ConfigPanel } from "./components/ConfigPanel";
import { ConsoleDrawer } from "./components/ConsoleDrawer";
import { PlayButton } from "./components/PlayButton";
import { ProgressHUD } from "./components/ProgressHUD";
import { SkinViewer } from "./components/SkinViewer";

const CONFIG_KEY = "kz.config.v1";
const MAX_LOG_LINES = 3000;
const PROGRESS_PHASES = new Set(["resolve", "loader", "java", "download", "kzengine"]);

const DEFAULT_CONFIG: LaunchOptions = {
  version: "",
  loader: "fabric",
  loaderVersion: null,
  kzEngine: true,
  ramMb: 4096,
};

function loadConfig(): LaunchOptions {
  try {
    const raw = localStorage.getItem(CONFIG_KEY);
    return raw ? { ...DEFAULT_CONFIG, ...(JSON.parse(raw) as Partial<LaunchOptions>) } : DEFAULT_CONFIG;
  } catch {
    return DEFAULT_CONFIG;
  }
}

/** Tampon de logs vidé à chaque frame pour ne pas re-rendre à chaque ligne. */
function useLogBuffer() {
  const [lines, setLines] = useState<LogLine[]>([]);
  const pending = useRef<LogLine[]>([]);
  const frame = useRef<number | null>(null);
  const push = useCallback((l: LogLine) => {
    pending.current.push(l);
    if (frame.current !== null) return;
    frame.current = requestAnimationFrame(() => {
      frame.current = null;
      const batch = pending.current;
      pending.current = [];
      setLines((cur) => {
        const next = cur.concat(batch);
        return next.length > MAX_LOG_LINES ? next.slice(next.length - MAX_LOG_LINES) : next;
      });
    });
  }, []);
  const clear = useCallback(() => setLines([]), []);
  return { lines, push, clear };
}

export default function App() {
  const reduceMotion = useReducedMotion();
  const [sys, setSys] = useState<SystemInfo | null>(null);
  const [versions, setVersions] = useState<VersionEntry[]>([]);
  const [accounts, setAccounts] = useState<AccountsView>({ accounts: [], active: null });
  const [config, setConfig] = useState<LaunchOptions>(loadConfig);
  const [phase, setPhase] = useState<PhaseEvent>({ phase: "idle", detail: "" });
  const [progress, setProgress] = useState<ProgressEvent | null>(null);
  const [skin, setSkin] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [loggingIn, setLoggingIn] = useState(false);
  const [consoleOpen, setConsoleOpen] = useState(false);
  const logs = useLogBuffer();

  const active = accounts.accounts.find((a) => a.uuid === accounts.active) ?? null;
  const locked = phase.phase !== "idle";

  // Chargement initial
  useEffect(() => {
    api.systemInfo().then((s) => {
      setSys(s);
      setConfig((c) => (localStorage.getItem(CONFIG_KEY) ? c : { ...c, ramMb: s.recommendedMb }));
    }).catch((e) => setError(errorText(e)));
    api.accounts().then(setAccounts).catch((e) => setError(errorText(e)));
    api.listVersions().then((list) => {
      setVersions(list);
      setConfig((c) => (c.version && list.some((v) => v.id === c.version) ? c : { ...c, version: list.find((v) => v.type === "release")?.id ?? "" }));
    }).catch((e) => setError(`Liste des versions indisponible : ${errorText(e)}`));
  }, []);

  // Évènements backend
  useEffect(() => {
    const offs = [
      on<PhaseEvent>("kz://phase", (p) => {
        setPhase(p);
        if (p.phase === "idle" || p.phase === "running") setProgress(null);
      }),
      on<ProgressEvent>("kz://progress", setProgress),
      on<LogLine>("kz://log", logs.push),
      on<ExitEvent>("kz://exit", (e) => {
        if (e.code !== null && e.code !== 0) {
          setError(`Minecraft s'est fermé avec le code ${e.code}. Ouvre la console pour le détail.`);
        }
      }),
      on<AuthWarning>("kz://auth-warning", (w) => setNotice(w.message)),
    ];
    return () => offs.forEach((off) => off());
  }, [logs.push]);

  // Persistance de la config (préférence locale)
  useEffect(() => {
    try {
      localStorage.setItem(CONFIG_KEY, JSON.stringify(config));
    } catch {
      /* stockage indisponible : la config reste en mémoire */
    }
  }, [config]);

  // Skin du compte actif
  useEffect(() => {
    let alive = true;
    skinData(active?.skinUrl ?? null)
      .then((s) => alive && setSkin(s))
      .catch(() => alive && setSkin(null));
    return () => {
      alive = false;
    };
  }, [active?.skinUrl]);

  // Disparition auto des messages
  useEffect(() => {
    if (!notice) return;
    const t = setTimeout(() => setNotice(null), 6000);
    return () => clearTimeout(t);
  }, [notice]);

  const login = async () => {
    setError(null);
    setLoggingIn(true);
    try {
      await api.login();
      setAccounts(await api.accounts());
    } catch (e) {
      setError(errorText(e));
    } finally {
      setLoggingIn(false);
    }
  };

  const switchAccount = async (uuid: string) => {
    try {
      await api.switchAccount(uuid);
      setAccounts(await api.accounts());
    } catch (e) {
      setError(errorText(e));
    }
  };

  const removeAccount = async (uuid: string) => {
    try {
      setAccounts(await api.removeAccount(uuid));
    } catch (e) {
      setError(errorText(e));
    }
  };

  const play = async () => {
    setError(null);
    logs.clear();
    try {
      await api.launch(config);
    } catch (e) {
      setError(errorText(e));
    }
  };

  const patch = (p: Partial<LaunchOptions>) => setConfig((c) => ({ ...c, ...p }));

  return (
    <div className="relative h-full overflow-hidden bg-base">
      {/* Fond animé */}
      <div aria-hidden="true" className="pointer-events-none absolute inset-0 overflow-hidden">
        <motion.div
          className="absolute -left-40 -top-40 h-[640px] w-[640px] rounded-full bg-kz-600/25 blur-[140px]"
          animate={reduceMotion ? undefined : { x: [0, 80, 0], y: [0, 40, 0] }}
          transition={{ duration: 18, repeat: Infinity, ease: "easeInOut" }}
        />
        <motion.div
          className="absolute -bottom-48 right-0 h-[560px] w-[560px] rounded-full bg-kz-cyan/15 blur-[140px]"
          animate={reduceMotion ? undefined : { x: [0, -60, 0], y: [0, -50, 0] }}
          transition={{ duration: 22, repeat: Infinity, ease: "easeInOut" }}
        />
        <div className="absolute inset-0 bg-[radial-gradient(ellipse_at_center,transparent_40%,#09090b_100%)]" />
      </div>

      <div className="relative z-10 flex h-full flex-col gap-4 p-4">
        {/* En-tête */}
        <header className="flex items-center justify-between px-2">
          <div className="flex items-center gap-3">
            <div className="grid h-10 w-10 place-items-center rounded-2xl bg-gradient-to-br from-kz-500 to-kz-cyan font-black text-white shadow-glow">
              KZ
            </div>
            <div>
              <h1 className="text-lg font-black tracking-[0.25em] text-white">KZ CLIENT</h1>
              <p className="text-[11px] text-zinc-500">Premium Minecraft Launcher</p>
            </div>
          </div>
          <button
            type="button"
            onClick={() => setConsoleOpen((o) => !o)}
            aria-expanded={consoleOpen}
            className="focus-ring glass glass-hover rounded-2xl px-4 py-2 text-xs font-semibold text-zinc-300"
          >
            Console {logs.lines.length > 0 && <span className="ml-1 text-kz-400">{logs.lines.length}</span>}
          </button>
        </header>

        <main className="grid min-h-0 flex-1 grid-cols-[280px_1fr_360px] gap-4">
          <AccountPanel
            view={accounts}
            loggingIn={loggingIn}
            locked={locked}
            onLogin={login}
            onSwitch={switchAccount}
            onRemove={removeAccount}
          />

          {/* Centre : skin + jouer */}
          <section className="glass relative flex min-h-0 flex-col items-center p-6" aria-label="Joueur">
            <div className="text-center">
              <p className="label">{active ? "Connecté" : "Hors ligne"}</p>
              <h2 className="mt-1 text-3xl font-black text-white">{active?.username ?? "Aucun compte"}</h2>
              {phase.phase === "running" && <p className="mt-1 text-sm text-emerald-400">{phase.detail}</p>}
            </div>
            <div className="relative min-h-0 w-full flex-1">
              <SkinViewer skin={skin} slim={active?.skinSlim ?? false} />
            </div>
            <div className="flex w-full flex-col items-center gap-3">
              <ProgressHUD progress={progress} visible={PROGRESS_PHASES.has(phase.phase)} detail={phase.detail} />
              <PlayButton phase={phase.phase} disabled={!active || !config.version} onClick={play} />
              {!active && <p className="text-xs text-zinc-500">Ajoute un compte Microsoft pour jouer.</p>}
            </div>
          </section>

          <ConfigPanel versions={versions} config={config} sys={sys} locked={locked} onChange={patch} />
        </main>
      </div>

      {/* Messages */}
      <div className="pointer-events-none fixed inset-x-0 top-4 z-50 flex flex-col items-center gap-2 px-4">
        <AnimatePresence>
          {error && (
            <motion.div
              key="error"
              role="alert"
              initial={{ opacity: 0, y: -16 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -16 }}
              className="pointer-events-auto flex max-w-2xl items-start gap-3 rounded-2xl border border-red-500/30 bg-red-950/70 px-4 py-3 text-sm text-red-100 backdrop-blur-xl"
            >
              <span className="flex-1 select-text">{error}</span>
              <button type="button" onClick={() => setError(null)} className="focus-ring rounded text-red-300 hover:text-white" aria-label="Fermer l'erreur">
                ✕
              </button>
            </motion.div>
          )}
          {notice && (
            <motion.div
              key="notice"
              role="status"
              initial={{ opacity: 0, y: -16 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -16 }}
              className="max-w-2xl rounded-2xl border border-amber-400/30 bg-amber-950/60 px-4 py-3 text-sm text-amber-100 backdrop-blur-xl"
            >
              {notice}
            </motion.div>
          )}
        </AnimatePresence>
      </div>

      <ConsoleDrawer open={consoleOpen} lines={logs.lines} onClose={() => setConsoleOpen(false)} />
    </div>
  );
}
