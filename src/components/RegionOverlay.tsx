import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";

interface Pt {
  x: number;
  y: number;
}

/**
 * 영역 선택 오버레이. 전체 화면을 채운 캡처 이미지 위에서 사각형을 드래그하면
 * 그 영역(뷰포트 논리 px)을 백엔드로 보내 크롭하게 한다. Esc/우클릭은 취소.
 */
export function RegionOverlay() {
  const [img, setImg] = useState<string | null>(null);
  const [start, setStart] = useState<Pt | null>(null);
  const [cur, setCur] = useState<Pt | null>(null);
  const doneRef = useRef(false);

  useEffect(() => {
    invoke<string | null>("region_get_image").then(setImg).catch(() => setImg(null));
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") cancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const cancel = () => {
    if (doneRef.current) return;
    doneRef.current = true;
    invoke("region_cancel").catch(() => {});
  };

  const rect =
    start && cur
      ? {
          x: Math.min(start.x, cur.x),
          y: Math.min(start.y, cur.y),
          w: Math.abs(cur.x - start.x),
          h: Math.abs(cur.y - start.y),
        }
      : null;

  const onMouseDown = (e: React.MouseEvent) => {
    if (e.button !== 0) {
      cancel();
      return;
    }
    setStart({ x: e.clientX, y: e.clientY });
    setCur({ x: e.clientX, y: e.clientY });
  };

  const onMouseMove = (e: React.MouseEvent) => {
    if (start) setCur({ x: e.clientX, y: e.clientY });
  };

  const onMouseUp = () => {
    if (!rect || doneRef.current) return;
    if (rect.w < 5 || rect.h < 5) {
      // 너무 작은 선택은 무시하고 다시 그리게 한다
      setStart(null);
      setCur(null);
      return;
    }
    doneRef.current = true;
    invoke("region_finish", {
      rect: {
        x: rect.x,
        y: rect.y,
        w: rect.w,
        h: rect.h,
        view_w: window.innerWidth,
        view_h: window.innerHeight,
      },
    }).catch(() => {});
  };

  return (
    <div
      className="region-overlay"
      onMouseDown={onMouseDown}
      onMouseMove={onMouseMove}
      onMouseUp={onMouseUp}
      onContextMenu={(e) => e.preventDefault()}
    >
      {img && <img className="region-img" src={img} alt="" draggable={false} />}
      {!rect && <div className="region-dim-full" />}
      {rect && (
        <div
          className="region-sel"
          style={{ left: rect.x, top: rect.y, width: rect.w, height: rect.h }}
        />
      )}
      <div className="region-hint">드래그해서 영역을 선택하세요 · Esc 취소</div>
    </div>
  );
}
