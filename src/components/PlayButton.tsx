import { AnimatePresence, motion } from "framer-motion";
import type { Phase } from "../lib/api";

const LABELS: Record<Phase, string> = {
  idle: "JOUER",
  auth: "Connexion...",
  resolve: "Préparation...",
  loader: "Installation du loader...",
  java: "Préparation de Java...",
  download: "Téléchargement...",
  kzengine: "Mise à jour KZ Engine...",
  launching: "Lancement...",
  running: "En jeu",
};

export function PlayButton({ phase, disabled, onClick }: { phase: Phase; disabled: boolean; onClick: () => void }) {
  const idle = phase === "idle";
  const running = phase === "running";
  const busy = !idle && !running;

  return (
    <motion.button
      type="button"
      onClick={onClick}
      disabled={disabled || !idle}
      aria-live="polite"
      whileHover={idle && !disabled ? { scale: 1.03, y: -2 } : undefined}
      whileTap={idle && !disabled ? { scale: 0.97 } : undefined}
      className={`focus-ring group relative h-20 w-full max-w-md overflow-hidden rounded-3xl font-black tracking-[0.2em] text-white transition-shadow disabled:cursor-not-allowed ${
        running
          ? "bg-emerald-500/20 ring-1 ring-emerald-400/40"
          : "bg-gradient-to-r from-kz-600 via-kz-500 to-kz-cyan shadow-glow"
      } ${disabled && idle ? "opacity-50 grayscale" : ""}`}
    >
      {idle && !disabled && (
        <motion.span
          aria-hidden="true"
          className="absolute inset-0 bg-gradient-to-r from-transparent via-white/25 to-transparent"
          initial={{ x: "-100%" }}
          animate={{ x: "100%" }}
          transition={{ duration: 2.4, repeat: Infinity, ease: "easeInOut", repeatDelay: 1.2 }}
        />
      )}
      {busy && (
        <motion.span
          aria-hidden="true"
          className="absolute inset-0 bg-[length:200%_100%] bg-gradient-to-r from-kz-600 via-kz-cyan to-kz-600"
          animate={{ backgroundPositionX: ["0%", "200%"] }}
          transition={{ duration: 2, repeat: Infinity, ease: "linear" }}
        />
      )}
      <AnimatePresence mode="wait">
        <motion.span
          key={phase}
          initial={{ y: 14, opacity: 0 }}
          animate={{ y: 0, opacity: 1 }}
          exit={{ y: -14, opacity: 0 }}
          transition={{ duration: 0.18 }}
          className={`relative flex items-center justify-center gap-3 ${idle ? "text-2xl" : "text-base tracking-[0.12em]"}`}
        >
          {running && <span className="h-2.5 w-2.5 animate-pulse rounded-full bg-emerald-400" aria-hidden="true" />}
          {LABELS[phase]}
        </motion.span>
      </AnimatePresence>
    </motion.button>
  );
}
