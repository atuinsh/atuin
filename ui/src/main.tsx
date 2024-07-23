import React from "react";
import ReactDOM from "react-dom/client";
import { NextUIProvider } from "@nextui-org/react";
import App from "./App";
import "./styles.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <NextUIProvider>
      <main className="text-foreground bg-background">
        <div
          data-tauri-drag-region
          className="w-full min-h-8 z-10 border-b-1"
        />

        <div className="z-20 ">
          <App />
        </div>
      </main>
    </NextUIProvider>
  </React.StrictMode>,
);
