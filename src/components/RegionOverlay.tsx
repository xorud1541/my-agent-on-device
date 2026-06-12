import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";

interface Pt {
  x: number;
  y: number;
}

interface Props {
  /** 전체 캡처 스크린샷 data URL */
  src: string;
  /** 캡처 원본의 캐시 경로 (crop_capture 입력) */
  fullPath: string;
  onDone: (att: { path: string; thumb: string }) => void;
  onCancel: () => void;
}

/**
 * 앱 내 전체화면 모달에서 캡처 스크린샷 위에 사각형을 드래그해 영역을 선택한다.
 * 별도 창을 만들지 않아(단일 webview) macOS WebKit 크래시를 피한다.
 * 선택 좌표는 렌더된 <img> 박스를 기준으로 원본 픽셀로 환산해 백엔드에 보낸다.
 */
export function RegionOverlay({ src, fullPath, onDone, onCancel }: Props) {
  const [start, setStart] = useState<Pt | null>(null);
  const [cur, setCur] = useState<Pt | null>(null);
  const [busy, setBusy] = useState(false);
  const imgRef = useRef<HTMLImageElement>(null);
  const doneRef = useRef(false);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") cancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const cancel = () => {
    if (doneRef.current) return;
    doneRef.current = true;
    onCancel();
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
    if (busy) return;
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

  const onMouseUp = async () => {
    if (!rect || busy || doneRef.current) return;
    if (rect.w < 5 || rect.h < 5) {
      setStart(null);
      setCur(null);
      return;
    }
    const img = imgRef.current;
    if (!img) return;
    const box = img.getBoundingClientRect();
    // 렌더된 이미지 박스 기준 → 정규화 비율(0~1). 프리뷰 해상도와 무관하게 원본에서 정확히 크롭됨.
    const clamp01 = (v: number) => Math.min(1, Math.max(0, v));
    const norm = {
      x: clamp01((rect.x - box.left) / box.width),
      y: clamp01((rect.y - box.top) / box.height),
      w: clamp01(rect.w / box.width),
      h: clamp01(rect.h / box.height),
    };
    setBusy(true);
    doneRef.current = true;
    try {
      const r = await invoke<{ path: string; thumb_data_url: string }>("crop_capture", {
        fullPath,
        rect: norm,
      });
      onDone({ path: r.path, thumb: r.thumb_data_url });
    } catch {
      // 실패 시 모달만 닫는다
      onCancel();
    }
  };

  return (
    <div
      className="region-overlay"
      onMouseDown={onMouseDown}
      onMouseMove={onMouseMove}
      onMouseUp={onMouseUp}
      onContextMenu={(e) => {
        e.preventDefault();
        cancel();
      }}
    >
      <img ref={imgRef} className="region-img" src={src} alt="" draggable={false} />
      {!rect && <div className="region-dim-full" />}
      {rect && (
        <div
          className="region-sel"
          style={{ left: rect.x, top: rect.y, width: rect.w, height: rect.h }}
        />
      )}
      <div className="region-hint">
        {busy ? "잘라내는 중…" : "드래그해서 영역을 선택하세요 · Esc/우클릭 취소"}
      </div>
    </div>
  );
}
