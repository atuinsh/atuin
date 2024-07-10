import React from "react";
import ReactDOM from "react-dom/client";
import { NextUIProvider, Spacer } from "@nextui-org/react";
import App from "./App";
import "./styles.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <NextUIProvider>
      <div data-tauri-drag-region className="w-full h-8 absolute" />
      <App />
    </NextUIProvider>
  </React.StrictMode>,
);
