import {
  SideMenuProps,
  useBlockNoteEditor,
  useComponentsContext,
} from "@blocknote/react";
import { TrashIcon } from "lucide-react";

// Custom Side Menu button to remove the hovered block.
export function DeleteBlock(props: SideMenuProps) {
  const editor = useBlockNoteEditor();

  const Components = useComponentsContext()!;

  return (
    <Components.SideMenu.Button
      label="Remove block"
      className="mx-1"
      icon={
        <TrashIcon
          size={24}
          onClick={() => {
            editor.removeBlocks([props.block]);
          }}
        />
      }
    />
  );
}
