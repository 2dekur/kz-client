import { useEffect, useRef } from "react";
import { AnimatePresence, motion } from "framer-motion";
import type { LogLine } from "../lib/api";

function color(l: LogLine) {
  const level = l.level?.toUpperCase();
  if (level === "ERROR" || level === "FATAL") return "text-red-400";
  if (level === "WARN") return "text-amber-300";
  if (l.stream === "stderr") return "text-red-300/80";
  return "text-zinc-400";
}

export function ConsoleDrawer({ open, lines, onClose }: { open: boolean; lines: LogLine[]; onClose: () => void }) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const stick = useRef(true);

  useEffect(() => {
    const el = scrollRef.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [lines, open]);

  return (
    <AnimatePresence>
      {open && (
        <motion.aside
          initial={{ y: "100%" }}
          animate={{ y: 0 }}
          exit={{ y: "100%" }}
          transition={{ type: "spring", stiffness: 320, damping: 34 }}
          className="fixed inset-x-4 bottom-4 z-40 flex h-[45vh] flex-col overflow-hidden rounded-3xl border border-white/10 bg-black/80 backdrop-blur-2xl"
          aria-label="Console du jeu"
        >
          <div className="flex items-center justify-between border-b border-white/10 px-5 py-3">
            <span className="label">Console · {lines.length} lignes</span>
            <button
              type="button"
              onClick={onClose}
              className="focus-ring rounded-lg px-2 py-1 text-xs text-zinc-400 hover:bg-white/10 hover:text-white"
            >
              Fermer
            </button>
          </div>
          <div
            ref={scrollRef}
            onScroll={(e) => {
              const el = e.currentTarget;
              stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
            }}
            className="flex-1 select-text overflow-y-auto px-5 py-3 font-mono text-[12px] leading-relaxed"
          >
            {lines.map((l, i) => (
              <div key={i} className={`whitespace-pre-wrap break-all ${color(l)}`}>
                {l.level && <span className="mr-2 opacity-60">[{l.level}]</span>}
                {l.line}
              </div>
            ))}
          </div>
        </motion.aside>
      )}
    </AnimatePresence>
  );
}
