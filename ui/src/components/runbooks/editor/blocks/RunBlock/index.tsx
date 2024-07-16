import { createReactBlockSpec } from "@blocknote/react";
import "./index.css";

import CodeMirror from "@uiw/react-codemirror";
import { langs } from "@uiw/codemirror-extensions-langs";

import { Play, Square } from "lucide-react";
import { useState } from "react";

import { extensions } from "./extensions";
import { invoke } from "@tauri-apps/api/core";
import Terminal from "./terminal.tsx";

import "@xterm/xterm/css/xterm.css";

interface RunBlockProps {
  onChange: (val: string) => void;
  onPlay?: () => void;
  onStop?: () => void;
  id: string;
  code: string;
  type: string;
  isEditable: boolean;
}

const RunBlock = ({
  onChange,
  onPlay,
  id,
  code,
  isEditable,
}: RunBlockProps) => {
  console.log(code);
  const [isRunning, setIsRunning] = useState(false);
  const [showTerminal, setShowTerminal] = useState(false);
  const [value, setValue] = useState<String>(code);

  const [pty, setPty] = useState<string | null>(null);

  const handleToggle = async (event: any) => {
    event.stopPropagation();

    // If there's no code, don't do anything
    if (!value) return;

    setIsRunning(!isRunning);
    setShowTerminal(!isRunning);

    if (isRunning) {
      // send sigkill
      console.log("sending sigkill");
      await invoke("pty_kill", { pid: pty });
    }

    if (!isRunning) {
      if (onPlay) onPlay();

      let pty = await invoke<string>("pty_open");
      setPty(pty);
      console.log(pty);

      let val = !value.endsWith("\n") ? value + "\r\n" : value;
      await invoke("pty_write", { pid: pty, data: val });
    }
  };

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
            extensions={[...extensions(), langs.shell()]}
            basicSetup={false}
          />
          <div
            className={`overflow-hidden transition-all duration-300 ease-in-out min-w-0 ${
              showTerminal ? "block" : "hidden"
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
    },
    content: "none",
  },
  {
    // @ts-ignore
    render: ({ block, editor, code, type }) => {
      const onInputChange = (val: string) => {
        editor.updateBlock(block, {
          props: { ...block.props, code: val },
        });
        console.log(block.props);
      };

      return (
        <RunBlock
          onChange={onInputChange}
          id={block?.id}
          code={block.props.code}
          type={block.props.type}
          isEditable={editor.isEditable}
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
