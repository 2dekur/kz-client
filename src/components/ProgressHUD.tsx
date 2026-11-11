import { useEffect } from "react";
import { AnimatePresence, motion, useSpring, useTransform } from "framer-motion";
import type { ProgressEvent } from "../lib/api";

const MB = 1024 * 1024;

/** Barre interpolée par ressort : rendu à la fréquence de l'écran (60/144/240 Hz). */
export function ProgressHUD({ progress, visible, detail }: { progress: ProgressEvent | null; visible: boolean; detail: string }) {
  const target = progress
    ? progress.bytesTotal > 0
      ? (progress.bytesDone / progress.bytesTotal) * 100
      : progress.filesTotal > 0
        ? (progress.filesDone / progress.filesTotal) * 100
        : 0
    : 0;
  const spring = useSpring(0, { stiffness: 140, damping: 26, mass: 0.6 });
  const scaleX = useTransform(spring, (v) => Math.min(1, Math.max(0, v / 100)));
  const percent = useTransform(spring, (v) => `${Math.min(100, Math.max(0, v)).toFixed(1)}%`);

  useEffect(() => {
    spring.set(Math.min(100, target));
  }, [spring, target]);

  const remaining = progress ? Math.max(0, progress.filesTotal - progress.filesDone) : 0;

  return (
    <AnimatePresence>
      {visible && progress && (
        <motion.div
          initial={{ opacity: 0, y: 24 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0, y: 24 }}
          className="glass w-full max-w-2xl p-4"
          role="progressbar"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={Math.round(target)}
          aria-label={progress.stage}
        >
          <div className="mb-2 flex items-baseline justify-between gap-4">
            <div className="min-w-0">
              <div className="truncate text-sm font-semibold text-zinc-100">{progress.stage}</div>
              <div className="truncate text-xs text-zinc-500">{detail}</div>
            </div>
            <motion.span className="font-mono text-2xl font-bold tabular-nums text-white">{percent}</motion.span>
          </div>
          <div className="h-2 overflow-hidden rounded-full bg-white/[0.06]">
            <motion.div
              className="h-full origin-left rounded-full bg-gradient-to-r from-kz-600 via-kz-400 to-kz-cyan"
              style={{ scaleX }}
            />
          </div>
          <div className="mt-2 flex justify-between font-mono text-[11px] tabular-nums text-zinc-400">
            <span>{(progress.speed / MB).toFixed(1)} Mo/s</span>
            <span>
              {(progress.bytesDone / MB).toFixed(0)} / {(progress.bytesTotal / MB).toFixed(0)} Mo
            </span>
            <span>
              {remaining} fichier{remaining > 1 ? "s" : ""} restant{remaining > 1 ? "s" : ""}
            </span>
          </div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
