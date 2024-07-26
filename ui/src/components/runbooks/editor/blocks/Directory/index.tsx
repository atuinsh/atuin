import { useState } from "react";
import { Input, Tooltip, Button } from "@nextui-org/react";
import { FolderInputIcon } from "lucide-react";

// @ts-ignore
import { createReactBlockSpec } from "@blocknote/react";

import { open } from "@tauri-apps/plugin-dialog";

interface DirectoryProps {
  path: string;
  onInputChange: (val: string) => void;
}

const Directory = ({ path, onInputChange }: DirectoryProps) => {
  const [value, setValue] = useState(path);

  const selectFolder = async () => {
    const path = await open({
      multiple: false,
      directory: true,
    });

    setValue(path || "");
    onInputChange(path || "");
  };

  return (
    <div className="w-full !max-w-full !outline-none overflow-none">
      <Tooltip
        content="Change working directory for all subsequent code blocks"
        delay={1000}
      >
        <div className="flex flex-row">
          <div className="mr-2">
            <Button
              isIconOnly
              variant="flat"
              aria-label="Select folder"
              onPress={selectFolder}
            >
              <FolderInputIcon />
            </Button>
          </div>

          <div className="w-full">
            <Input
              placeholder="~"
              value={value}
              autoComplete="off"
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck="false"
              onValueChange={(val) => {
                setValue(val);
                onInputChange(val);
              }}
            />
          </div>
        </div>
      </Tooltip>
    </div>
  );
};

export default createReactBlockSpec(
  {
    type: "directory",
    propSchema: {
      path: { default: "" },
    },
    content: "none",
  },
  {
    // @ts-ignore
    render: ({ block, editor, code, type }) => {
      const onInputChange = (val: string) => {
        editor.updateBlock(block, {
          // @ts-ignore
          props: { ...block.props, path: val },
        });
      };

      return (
        <Directory path={block.props.path} onInputChange={onInputChange} />
      );
    },
  },
);
