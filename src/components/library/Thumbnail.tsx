import { useEffect, useRef, useState } from "react";
import { Film } from "lucide-react";
import { thumbnailUrl } from "../../lib/replay";

/**
 * The session's thumbnail, loaded when it scrolls into view; an icon when there is none.
 * Fetched and shown as a blob URL: WebKitGTK never requests a `replay://` URL in an `<img src>`
 * (from the dev server's origin), while `fetch` works as for the preview segments.
 */
export function Thumbnail({ id, empty }: { id: string; empty: boolean }) {
  const ref = useRef<HTMLSpanElement>(null);
  const [src, setSrc] = useState<string | null>(null);
  const [failed, setFailed] = useState(empty);

  useEffect(() => {
    const el = ref.current;
    if (empty || !el) return;
    let active = true;
    let url: string | null = null;
    const io = new IntersectionObserver(
      ([entry]) => {
        if (!entry.isIntersecting) return;
        io.disconnect();
        fetch(thumbnailUrl(id))
          .then((r) => (r.ok ? r.blob() : Promise.reject(new Error(`${r.status}`))))
          .then((blob) => {
            if (!active) return;
            url = URL.createObjectURL(blob);
            setSrc(url);
          })
          .catch(() => active && setFailed(true));
      },
      { rootMargin: "200px" },
    );
    io.observe(el);
    return () => {
      active = false;
      io.disconnect();
      if (url) URL.revokeObjectURL(url);
    };
  }, [id, empty]);

  return (
    <span ref={ref} className={`thumb ${src ? "thumb-ok" : failed ? "thumb-failed" : "thumb-loading"}`}>
      {src && <img src={src} alt="" decoding="async" />}
      {!src && <Film size={18} aria-hidden />}
    </span>
  );
}
