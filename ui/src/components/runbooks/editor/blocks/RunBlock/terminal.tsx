/*
export const openTerm = (pty: string, id: string) => {
  const term = new Terminal({
    fontSize: 12,
    fontFamily: "Courier New",
  });

  let element = document.getElementById(id);
  term.open(element);

  //term.onResize(onResize(pty));

  //const fitAddon = new FitAddon();
  //term.loadAddon(fitAddon);
  //term.loadAddon(new WebglAddon());

  /*
  const onSize = (e) => {
    e.stopPropagation();
    fitAddon.fit();
  };
  fitAddon.fit();

  window.addEventListener("resize", onSize, false);
  */

import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { listen } from "@tauri-apps/api/event";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import "@xterm/xterm/css/xterm.css";
import { invoke } from "@tauri-apps/api/core";

const onResize = (pty: string) => async (size: any) => {
  await invoke("pty_resize", {
    pid: pty,
    cols: size.cols,
    rows: size.rows,
  });
};

const TerminalComponent = ({ pty }: any) => {
  const terminalRef = useRef(null);

  useEffect(() => {
    if (pty == null) return;
    if (!terminalRef.current) return;

    const terminal = new Terminal();
    const fitAddon = new FitAddon();

    terminal.open(terminalRef.current);
    terminal.loadAddon(new WebglAddon());
    terminal.loadAddon(fitAddon);
    terminal.onResize(onResize(pty));

    fitAddon.fit();
    const windowResize = () => {
      fitAddon.fit();
    };

    listen(`pty-${pty}`, (event: any) => {
      terminal.write(event.payload);
    }).then(() => {
      console.log("Listening for pty events");
    });

    window.addEventListener("resize", windowResize);

    // Customize further as needed
    return () => {
      terminal.dispose();
      window.removeEventListener("resize", windowResize);
    };
  }, [pty]);

  return (
    <div className="!max-w-full min-w-0 overflow-hidden" ref={terminalRef} />
  );
};

export default TerminalComponent;
