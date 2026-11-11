import { useMemo, useState } from "react";
import { motion } from "framer-motion";
import { formatGb, type LaunchOptions, type Loader, type SystemInfo, type VersionEntry, type VersionType } from "../lib/api";

const LOADERS: { id: Loader; label: string }[] = [
  { id: "vanilla", label: "Vanilla" },
  { id: "fabric", label: "Fabric" },
  { id: "quilt", label: "Quilt" },
  { id: "forge", label: "Forge" },
  { id: "neoforge", label: "NeoForge" },
];

const TYPES: { id: VersionType; label: string }[] = [
  { id: "release", label: "Releases" },
  { id: "snapshot", label: "Snapshots" },
  { id: "old_beta", label: "Beta" },
  { id: "old_alpha", label: "Alpha" },
];

const ENGINE_MODS: Record<Loader, string> = {
  vanilla: "Nécessite un modloader",
  fabric: "Sodium · Iris · Lithium · FerriteCore · ModMenu",
  quilt: "Sodium · Iris · Lithium · FerriteCore · ModMenu",
  forge: "Embeddium · FerriteCore",
  neoforge: "Sodium/Embeddium · Iris · Lithium · FerriteCore",
};

interface Props {
  versions: VersionEntry[];
  config: LaunchOptions;
  sys: SystemInfo | null;
  locked: boolean;
  onChange: (patch: Partial<LaunchOptions>) => void;
}

export function ConfigPanel({ versions, config, sys, locked, onChange }: Props) {
  const [types, setTypes] = useState<VersionType[]>(["release"]);
  const filtered = useMemo(() => {
    const list = versions.filter((v) => types.includes(v.type));
    if (!list.some((v) => v.id === config.version)) {
      const current = versions.find((v) => v.id === config.version);
      if (current) list.unshift(current);
    }
    return list;
  }, [versions, types, config.version]);

  const maxMb = sys?.maxMb ?? 16384;
  const fill = ((config.ramMb - 1024) / Math.max(1, maxMb - 1024)) * 100;
  const engineAvailable = config.loader !== "vanilla";

  const toggleType = (t: VersionType) =>
    setTypes((cur) => (cur.includes(t) ? (cur.length > 1 ? cur.filter((x) => x !== t) : cur) : [...cur, t]));

  return (
    <section className="glass flex h-full flex-col gap-6 overflow-y-auto p-5" aria-labelledby="config-title">
      <h2 id="config-title" className="label">
        Configuration
      </h2>

      {/* 1. Version */}
      <div className="space-y-3">
        <label htmlFor="version" className="text-sm font-medium text-zinc-300">
          Version
        </label>
        <div className="flex flex-wrap gap-1.5" role="group" aria-label="Types de versions affichés">
          {TYPES.map((t) => (
            <button
              key={t.id}
              type="button"
              onClick={() => toggleType(t.id)}
              aria-pressed={types.includes(t.id)}
              className={`focus-ring rounded-full border px-2.5 py-1 text-xs transition-colors ${
                types.includes(t.id)
                  ? "border-kz-400/50 bg-kz-500/15 text-kz-300"
                  : "border-white/10 text-zinc-500 hover:text-zinc-300"
              }`}
            >
              {t.label}
            </button>
          ))}
        </div>
        <select
          id="version"
          disabled={locked || filtered.length === 0}
          value={config.version}
          onChange={(e) => onChange({ version: e.target.value })}
          className="kz-select focus-ring w-full rounded-2xl border border-white/10 bg-white/[0.04] px-4 py-3 text-sm font-medium text-zinc-100 disabled:opacity-50"
        >
          {filtered.length === 0 && <option>Chargement des versions...</option>}
          {filtered.map((v) => (
            <option key={v.id} value={v.id}>
              {v.id}
              {v.type !== "release" ? `  (${v.type.replace("old_", "")})` : ""}
            </option>
          ))}
        </select>
      </div>

      {/* 2. Modloader */}
      <div className="space-y-3">
        <span id="loader-label" className="text-sm font-medium text-zinc-300">
          Modloader
        </span>
        <div className="grid grid-cols-5 gap-1 rounded-2xl border border-white/10 bg-white/[0.02] p-1" role="radiogroup" aria-labelledby="loader-label">
          {LOADERS.map((l) => {
            const active = config.loader === l.id;
            return (
              <button
                key={l.id}
                type="button"
                role="radio"
                aria-checked={active}
                disabled={locked}
                onClick={() => onChange({ loader: l.id, loaderVersion: null })}
                className={`focus-ring relative rounded-xl px-1 py-2 text-xs font-semibold transition-colors disabled:cursor-not-allowed ${
                  active ? "text-white" : "text-zinc-500 hover:text-zinc-300"
                }`}
              >
                {active && (
                  <motion.span
                    layoutId="loader-pill"
                    className="absolute inset-0 rounded-xl bg-gradient-to-br from-kz-500/40 to-kz-cyan/20 ring-1 ring-kz-400/40"
                    transition={{ type: "spring", stiffness: 500, damping: 38 }}
                  />
                )}
                <span className="relative">{l.label}</span>
              </button>
            );
          })}
        </div>
      </div>

      {/* 3. KZ Engine */}
      <button
        type="button"
        role="switch"
        aria-checked={engineAvailable && config.kzEngine}
        disabled={locked || !engineAvailable}
        onClick={() => onChange({ kzEngine: !config.kzEngine })}
        className={`focus-ring group flex items-center gap-4 rounded-2xl border p-4 text-left transition-all disabled:cursor-not-allowed ${
          engineAvailable && config.kzEngine
            ? "border-kz-400/40 bg-gradient-to-br from-kz-500/15 to-kz-cyan/5"
            : "border-white/10 bg-white/[0.02]"
        } ${!engineAvailable ? "opacity-50" : ""}`}
      >
        <div className="flex-1">
          <div className="flex items-center gap-2 text-sm font-bold text-zinc-100">
            <span className="bg-gradient-to-r from-kz-300 to-kz-cyan bg-clip-text text-transparent">KZ</span>
            Performance Engine
          </div>
          <div className="mt-1 text-xs text-zinc-500">{ENGINE_MODS[config.loader]}</div>
        </div>
        <span
          className={`relative h-7 w-12 shrink-0 rounded-full transition-colors ${
            engineAvailable && config.kzEngine ? "bg-kz-500" : "bg-white/10"
          }`}
        >
          <motion.span
            className="absolute top-1 h-5 w-5 rounded-full bg-white shadow"
            animate={{ left: engineAvailable && config.kzEngine ? 24 : 4 }}
            transition={{ type: "spring", stiffness: 600, damping: 32 }}
          />
        </span>
      </button>

      {/* 4. RAM */}
      <div className="space-y-3">
        <div className="flex items-baseline justify-between">
          <label htmlFor="ram" className="text-sm font-medium text-zinc-300">
            Mémoire allouée
          </label>
          <span className="font-mono text-lg font-bold tabular-nums text-zinc-100">{formatGb(config.ramMb)}</span>
        </div>
        <input
          id="ram"
          type="range"
          min={1024}
          max={maxMb}
          step={512}
          value={Math.min(config.ramMb, maxMb)}
          disabled={locked}
          onChange={(e) => onChange({ ramMb: Number(e.target.value) })}
          className="kz-range focus-ring w-full disabled:opacity-50"
          style={{ ["--fill" as string]: `${fill}%` }}
          aria-valuetext={formatGb(config.ramMb)}
        />
        <div className="flex justify-between text-[11px] text-zinc-500">
          <span>1 Go</span>
          {sys && (
            <button
              type="button"
              disabled={locked}
              onClick={() => onChange({ ramMb: sys.recommendedMb })}
              className="focus-ring rounded text-kz-400 hover:text-kz-300"
            >
              Recommandé : {formatGb(sys.recommendedMb)}
            </button>
          )}
          <span>{formatGb(maxMb)}</span>
        </div>
      </div>
    </section>
  );
}
