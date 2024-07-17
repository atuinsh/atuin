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

import { useState, useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { listen } from "@tauri-apps/api/event";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import "@xterm/xterm/css/xterm.css";
import { invoke } from "@tauri-apps/api/core";
import { useStore } from "@/state/store";

const onResize = (pty: string) => async (size: any) => {
  await invoke("pty_resize", {
    pid: pty,
    cols: size.cols,
    rows: size.rows,
  });
};

const usePersistentTerminal = (pty: string) => {
  const setPtyTerm = useStore((store) => store.setPtyTerm);
  const terminals = useStore((store) => store.terminals);
  const [isReady, setIsReady] = useState(false);

  useEffect(() => {
    if (!terminals.hasOwnProperty(pty)) {
      let terminal = new Terminal();
      setPtyTerm(pty, terminal);
    }

    setIsReady(true);

    return () => {
      // We don't dispose of the terminal when the component unmounts
    };
  }, [pty, terminals, setPtyTerm]);

  return { terminal: terminals[pty], isReady };
};

const TerminalComponent = ({ pty }: any) => {
  const terminalRef = useRef(null);
  const { terminal, isReady } = usePersistentTerminal(pty);
  const [isAttached, setIsAttached] = useState(false);
  const cleanupListenerRef = useRef<(() => void) | null>(null);

  useEffect(() => {
    if (pty == null) return;
    if (!isReady) return;

    if (!isAttached) {
      if (terminal && !terminal.element) {
        terminal.open(terminalRef.current);
      } else {
        terminalRef.current.appendChild(terminal.element);
      }
      setIsAttached(true);
    }

    listen(`pty-${pty}`, (event: any) => {
      terminal.write(event.payload);
    }).then((ul) => {
      cleanupListenerRef.current = ul;
    });

    //window.addEventListener("resize", windowResize);

    // Customize further as needed
    return () => {
      if (terminal && terminal.element) {
        // Instead of removing, we just detach
        if (terminal.element.parentElement) {
          terminal.element.parentElement.removeChild(terminal.element);
        }
        setIsAttached(false);
      }

      if (cleanupListenerRef.current) {
        cleanupListenerRef.current();
      }
    };
  }, [terminal, isReady]);

  if (!isReady) return null;

  return (
    <div className="!max-w-full min-w-0 overflow-hidden" ref={terminalRef} />
  );
};

export default TerminalComponent;
