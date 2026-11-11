import { useEffect, useRef, useState } from "react";
import { motion } from "framer-motion";
import * as skinview3d from "skinview3d";

type Anim = "idle" | "walk" | "run";

const ANIMS: { id: Anim; label: string }[] = [
  { id: "idle", label: "Idle" },
  { id: "walk", label: "Marche" },
  { id: "run", label: "Course" },
];

function makeAnimation(kind: Anim) {
  switch (kind) {
    case "idle":
      return new skinview3d.IdleAnimation();
    case "run": {
      const a = new skinview3d.RunningAnimation();
      a.speed = 0.7;
      return a;
    }
    default: {
      const a = new skinview3d.WalkingAnimation();
      a.speed = 0.6;
      return a;
    }
  }
}

/** Visualiseur 3D interactif (drag pour tourner, rotation auto). */
export function SkinViewer({ skin, slim }: { skin: string | null; slim: boolean }) {
  const wrapRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const viewerRef = useRef<skinview3d.SkinViewer | null>(null);
  const [anim, setAnim] = useState<Anim>("walk");
  const [ready, setReady] = useState(false);

  useEffect(() => {
    const canvas = canvasRef.current;
    const wrap = wrapRef.current;
    if (!canvas || !wrap) return;
    const viewer = new skinview3d.SkinViewer({
      canvas,
      width: wrap.clientWidth,
      height: wrap.clientHeight,
    });
    viewer.fov = 40;
    viewer.zoom = 0.82;
    viewer.autoRotate = true;
    viewer.autoRotateSpeed = 0.55;
    viewer.controls.enableZoom = false;
    viewer.controls.enablePan = false;
    viewerRef.current = viewer;

    const ro = new ResizeObserver(() => {
      viewer.width = wrap.clientWidth;
      viewer.height = wrap.clientHeight;
    });
    ro.observe(wrap);
    return () => {
      ro.disconnect();
      viewer.dispose();
      viewerRef.current = null;
    };
  }, []);

  useEffect(() => {
    const viewer = viewerRef.current;
    if (viewer) viewer.animation = makeAnimation(anim);
  }, [anim]);

  useEffect(() => {
    const viewer = viewerRef.current;
    if (!viewer || !skin) return;
    setReady(false);
    Promise.resolve(viewer.loadSkin(skin, { model: slim ? "slim" : "default" }))
      .then(() => setReady(true))
      .catch(() => setReady(true));
  }, [skin, slim]);

  return (
    <div className="relative h-full w-full">
      <div
        ref={wrapRef}
        className="absolute inset-0"
        role="img"
        aria-label="Aperçu 3D du skin du joueur, glisser pour faire pivoter"
      >
        <motion.canvas
          ref={canvasRef}
          className="h-full w-full cursor-grab active:cursor-grabbing"
          initial={{ opacity: 0, scale: 0.96 }}
          animate={{ opacity: ready ? 1 : 0.35, scale: 1 }}
          transition={{ duration: 0.6, ease: [0.22, 1, 0.36, 1] }}
        />
      </div>
      <div className="absolute bottom-3 left-1/2 flex -translate-x-1/2 gap-1 rounded-full border border-white/10 bg-black/40 p-1 backdrop-blur-xl">
        {ANIMS.map((a) => (
          <button
            key={a.id}
            type="button"
            onClick={() => setAnim(a.id)}
            aria-pressed={anim === a.id}
            className={`focus-ring relative rounded-full px-3 py-1 text-xs font-medium transition-colors ${
              anim === a.id ? "text-white" : "text-zinc-400 hover:text-zinc-200"
            }`}
          >
            {anim === a.id && (
              <motion.span
                layoutId="anim-pill"
                className="absolute inset-0 rounded-full bg-white/10"
                transition={{ type: "spring", stiffness: 500, damping: 35 }}
              />
            )}
            <span className="relative">{a.label}</span>
          </button>
        ))}
      </div>
    </div>
  );
}
