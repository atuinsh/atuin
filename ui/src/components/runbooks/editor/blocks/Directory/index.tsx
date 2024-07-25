import { useState } from "react";
import { Input, Tooltip } from "@nextui-org/react";
import { FolderInputIcon, HelpCircleIcon } from "lucide-react";

// @ts-ignore
import { createReactBlockSpec } from "@blocknote/react";

interface DirectoryProps {
  path: string;
  onInputChange: (string) => void;
}

const Directory = ({ path, onInputChange }: DirectoryProps) => {
  const [value, setValue] = useState(path);

  return (
    <div className="w-full !max-w-full !outline-none overflow-none">
      <Tooltip
        content="Change working directory for all subsequent code blocks"
        delay={1000}
      >
        <Input
          label="Directory"
          placeholder="~"
          labelPlacement="outside"
          value={value}
          autoComplete="off"
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck="false"
          onValueChange={(val) => {
            setValue(val);
            onInputChange(val);
          }}
          startContent={<FolderInputIcon />}
        />
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
