import { AnimatePresence, motion } from "framer-motion";
import type { AccountsView } from "../lib/api";
import { Avatar } from "./Avatar";

interface Props {
  view: AccountsView;
  loggingIn: boolean;
  locked: boolean;
  onLogin: () => void;
  onSwitch: (uuid: string) => void;
  onRemove: (uuid: string) => void;
}

export function AccountPanel({ view, loggingIn, locked, onLogin, onSwitch, onRemove }: Props) {
  return (
    <section className="glass flex h-full flex-col p-5" aria-labelledby="accounts-title">
      <h2 id="accounts-title" className="label mb-4">
        Comptes
      </h2>

      <ul className="flex-1 space-y-2 overflow-y-auto pr-1">
        <AnimatePresence initial={false}>
          {view.accounts.map((a) => {
            const active = a.uuid === view.active;
            return (
              <motion.li
                key={a.uuid}
                layout
                initial={{ opacity: 0, y: 8 }}
                animate={{ opacity: 1, y: 0 }}
                exit={{ opacity: 0, x: -16 }}
                className="group relative"
              >
                <button
                  type="button"
                  disabled={locked}
                  onClick={() => onSwitch(a.uuid)}
                  aria-pressed={active}
                  className={`focus-ring flex w-full items-center gap-3 rounded-2xl border p-2.5 pr-10 text-left transition-all disabled:cursor-not-allowed ${
                    active
                      ? "border-kz-400/50 bg-kz-500/10 shadow-glow"
                      : "border-white/5 bg-white/[0.02] hover:border-white/15 hover:bg-white/[0.05]"
                  }`}
                >
                  <Avatar skinUrl={a.skinUrl} label={`Avatar de ${a.username}`} />
                  <div className="min-w-0">
                    <div className="truncate font-semibold text-zinc-100">{a.username}</div>
                    <div className="text-xs text-zinc-500">{active ? "Actif" : "Microsoft"}</div>
                  </div>
                </button>
                <button
                  type="button"
                  disabled={locked}
                  onClick={() => onRemove(a.uuid)}
                  aria-label={`Retirer le compte ${a.username}`}
                  className="focus-ring absolute right-2 top-1/2 -translate-y-1/2 rounded-lg p-1.5 text-zinc-500 opacity-0 transition hover:bg-red-500/10 hover:text-red-400 focus-visible:opacity-100 group-hover:opacity-100 disabled:hidden"
                >
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" aria-hidden="true">
                    <path d="M18 6 6 18M6 6l12 12" strokeLinecap="round" />
                  </svg>
                </button>
              </motion.li>
            );
          })}
        </AnimatePresence>
        {view.accounts.length === 0 && (
          <li className="rounded-2xl border border-dashed border-white/10 p-4 text-sm text-zinc-500">
            Aucun compte. Connecte ton compte Microsoft pour jouer.
          </li>
        )}
      </ul>

      <motion.button
        type="button"
        onClick={onLogin}
        disabled={loggingIn || locked}
        whileHover={{ scale: 1.02 }}
        whileTap={{ scale: 0.98 }}
        className="focus-ring mt-4 flex items-center justify-center gap-2 rounded-2xl border border-white/10 bg-white/[0.06] px-4 py-3 text-sm font-semibold text-zinc-100 transition-colors hover:bg-white/10 disabled:cursor-wait disabled:opacity-60"
      >
        <svg width="16" height="16" viewBox="0 0 23 23" aria-hidden="true">
          <path fill="#f35325" d="M1 1h10v10H1z" />
          <path fill="#81bc06" d="M12 1h10v10H12z" />
          <path fill="#05a6f0" d="M1 12h10v10H1z" />
          <path fill="#ffba08" d="M12 12h10v10H12z" />
        </svg>
        {loggingIn ? "Connexion dans le navigateur..." : "Ajouter un compte Microsoft"}
      </motion.button>
    </section>
  );
}
