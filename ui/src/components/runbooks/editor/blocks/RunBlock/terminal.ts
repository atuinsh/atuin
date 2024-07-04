import { invoke } from "@tauri-apps/api/core";

import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";

const onResize = (pty: string) => async (size) => {
  await invoke("pty_resize", {
    pid: pty,
    cols: size.cols,
    rows: size.rows,
  });
};

export const openTerm = (pty: string, id: string) => {
  const term = new Terminal({
    fontSize: 12,
    fontFamily: "Courier New",
  });

  term.open(document.getElementById(id));

  term.onResize(onResize(pty));

  const fitAddon = new FitAddon();
  term.loadAddon(fitAddon);
  term.loadAddon(new WebglAddon());

  const onSize = () => {
    fitAddon.fit();
  };

  onSize();

  window.addEventListener("resize", onSize, false);

  return term;
};
