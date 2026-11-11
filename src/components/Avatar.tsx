import { useEffect, useState } from "react";
import { headFromSkin, skinData } from "../lib/api";

/** Tête pixel-art du joueur générée localement depuis sa texture. */
export function Avatar({ skinUrl, size = 40, label }: { skinUrl: string | null; size?: number; label: string }) {
  const [src, setSrc] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    skinData(skinUrl)
      .then((data) => headFromSkin(data))
      .then((head) => alive && setSrc(head))
      .catch(() => alive && setSrc(null));
    return () => {
      alive = false;
    };
  }, [skinUrl]);

  return (
    <div
      className="shrink-0 overflow-hidden rounded-xl bg-gradient-to-br from-kz-500/40 to-kz-cyan/30 ring-1 ring-white/10"
      style={{ width: size, height: size }}
    >
      {src && (
        <img src={src} alt={label} width={size} height={size} className="h-full w-full [image-rendering:pixelated]" />
      )}
    </div>
  );
}
