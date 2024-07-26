// @ts-ignore
import { createReactBlockSpec } from "@blocknote/react";

import "./index.css";

import CodeMirror from "@uiw/react-codemirror";
import { keymap } from "@codemirror/view";
import { langs } from "@uiw/codemirror-extensions-langs";

import { Play, Square } from "lucide-react";
import { useState } from "react";

import { extensions } from "./extensions";
import { platform } from "@tauri-apps/plugin-os";
import { invoke } from "@tauri-apps/api/core";
import Terminal from "./terminal.tsx";

import "@xterm/xterm/css/xterm.css";
import { AtuinState, useStore } from "@/state/store.ts";

interface RunBlockProps {
  onChange: (val: string) => void;
  onRun?: (pty: string) => void;
  onStop?: (pty: string) => void;
  id: string;
  code: string;
  type: string;
  pty: string;
  isEditable: boolean;
  editor: any;
}

const findFirstParentOfType = (editor: any, id: string, type: string): any => {
  // TODO: the types for blocknote aren't working. Now I'm doing this sort of shit,
  // really need to fix that.
  const document = editor.document;
  var lastOfType = null;

  // Iterate through ALL of the blocks.
  for (let i = 0; i < document.length; i++) {
    if (document[i].id == id) return lastOfType;

    if (document[i].type == type) lastOfType = document[i];
  }

  return lastOfType;
};

const RunBlock = ({
  onChange,
  id,
  code,
  isEditable,
  onRun,
  onStop,
  pty,
  editor,
}: RunBlockProps) => {
  const [value, setValue] = useState<String>(code);
  const cleanupPtyTerm = useStore((store: AtuinState) => store.cleanupPtyTerm);
  const terminals = useStore((store: AtuinState) => store.terminals);

  const [currentRunbook, incRunbookPty, decRunbookPty] = useStore(
    (store: AtuinState) => [
      store.currentRunbook,
      store.incRunbookPty,
      store.decRunbookPty,
    ],
  );

  const isRunning = pty !== null && pty !== "";

  const handleToggle = async (event: any | null) => {
    if (event) event.stopPropagation();

    // If there's no code, don't do anything
    if (!value) return;

    if (isRunning) {
      await invoke("pty_kill", { pid: pty });

      terminals[pty].terminal.dispose();
      cleanupPtyTerm(pty);

      if (onStop) onStop(pty);
      if (currentRunbook) decRunbookPty(currentRunbook);
    }

    if (!isRunning) {
      let cwd = findFirstParentOfType(editor, id, "directory");

      if (cwd) {
        cwd = cwd.props.path;
      } else {
        cwd = "~";
      }

      let pty = await invoke<string>("pty_open", { cwd });
      if (onRun) onRun(pty);

      if (currentRunbook) incRunbookPty(currentRunbook);

      let isWindows = platform() == "windows";
      let cmdEnd = isWindows ? "\r\n" : "\n";

      let val = !value.endsWith("\n") ? value + cmdEnd : value;
      await invoke("pty_write", { pid: pty, data: val });
    }
  };

  const handleCmdEnter = () => {
    handleToggle(null);
    return true;
  };

  const customKeymap = keymap.of([
    {
      key: "Mod-Enter",
      run: handleCmdEnter,
    },
  ]);

  return (
    <div className="w-full !max-w-full !outline-none overflow-none">
      <div className="flex flex-row items-start">
        <div className="flex">
          <button
            onClick={handleToggle}
            className={`flex items-center justify-center flex-shrink-0 w-8 h-8 mr-2 rounded border focus:outline-none focus:ring-2 transition-all duration-300 ease-in-out ${
              isRunning
                ? "border-red-200 bg-red-50 text-red-600 hover:bg-red-100 hover:border-red-300 focus:ring-red-300"
                : "border-green-200 bg-green-50 text-green-600 hover:bg-green-100 hover:border-green-300 focus:ring-green-300"
            }`}
            aria-label={isRunning ? "Stop code" : "Run code"}
          >
            <span
              className={`inline-block transition-transform duration-300 ease-in-out ${isRunning ? "rotate-180" : ""}`}
            >
              {isRunning ? <Square size={16} /> : <Play size={16} />}
            </span>
          </button>
        </div>
        <div className="flex-1 min-w-0 w-40">
          <CodeMirror
            id={id}
            placeholder={"Write your code here..."}
            className="!pt-0 max-w-full border border-gray-300 rounded"
            value={code}
            editable={isEditable}
            autoFocus
            onChange={(val) => {
              setValue(val);
              onChange(val);
            }}
            extensions={[customKeymap, ...extensions(), langs.shell()]}
            basicSetup={false}
          />
          <div
            className={`overflow-hidden transition-all duration-300 ease-in-out min-w-0 ${
              isRunning ? "block" : "hidden"
            }`}
          >
            {pty && <Terminal pty={pty} />}
          </div>
        </div>
      </div>
    </div>
  );
};

export default createReactBlockSpec(
  {
    type: "run",
    propSchema: {
      type: {
        default: "bash",
      },
      code: { default: "" },
      pty: { default: "" },
      global: { default: false },
    },
    content: "none",
  },
  {
    // @ts-ignore
    render: ({ block, editor, code, type }) => {
      const onInputChange = (val: string) => {
        editor.updateBlock(block, {
          // @ts-ignore
          props: { ...block.props, code: val },
        });
      };

      const onRun = (pty: string) => {
        editor.updateBlock(block, {
          // @ts-ignore
          props: { ...block.props, pty: pty },
        });
      };

      const onStop = (_pty: string) => {
        editor?.updateBlock(block, {
          props: { ...block.props, pty: "" },
        });
      };

      return (
        <RunBlock
          onChange={onInputChange}
          id={block?.id}
          code={block.props.code}
          type={block.props.type}
          pty={block.props.pty}
          isEditable={editor.isEditable}
          onRun={onRun}
          onStop={onStop}
          editor={editor}
        />
      );
    },
    toExternalHTML: ({ block }) => {
      return (
        <pre lang="beep boop">
          <code lang="bash">{block?.props?.code}</code>
        </pre>
      );
    },
  },
);
