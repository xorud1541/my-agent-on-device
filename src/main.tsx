import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { RegionOverlay } from "./components/RegionOverlay";
import "./styles/global.css";

// 영역 선택 오버레이 창은 같은 번들을 ?overlay=1 로 로드한다
const isOverlay = new URLSearchParams(window.location.search).get("overlay") === "1";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>{isOverlay ? <RegionOverlay /> : <App />}</React.StrictMode>,
);
