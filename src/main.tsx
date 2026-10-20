import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import "./index.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);

// 仅开发模式：URL 带 ?e2e=full 时自动跑一遍真实 UI 流程。
// 生产构建中 DEV 为 false，动态导入会被摇树掉，不会进入产物。
if (import.meta.env.DEV) {
  const scenario = new URLSearchParams(window.location.search).get("e2e");
  if (scenario) {
    import("./lib/e2e").then((m) => m.runE2E(scenario));
  }
}
