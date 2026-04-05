import { initDesktopBootstrap } from "./desktop/bootstrap";
import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles.css";

async function bootstrap(): Promise<void> {
  await initDesktopBootstrap();

  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
}

void bootstrap();
