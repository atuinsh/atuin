import "@blocknote/core/fonts/inter.css";
import "@blocknote/mantine/style.css";
import "./index.css";

import {
  BlockNoteSchema,
  defaultBlockSpecs,
  filterSuggestionItems,
  insertOrUpdateBlock,
} from "@blocknote/core";
import "@blocknote/core/fonts/inter.css";

import {
  SuggestionMenuController,
  AddBlockButton,
  getDefaultReactSlashMenuItems,
  useCreateBlockNote,
  SideMenu,
  SideMenuController,
} from "@blocknote/react";
import { BlockNoteView } from "@blocknote/mantine";

import { Code } from "lucide-react";

import RunBlock from "@/components/runbooks/editor/blocks/RunBlock";
import { DeleteBlock } from "@/components/runbooks/editor/ui/DeleteBlockButton";

// Our schema with block specs, which contain the configs and implementations for blocks
// that we want our editor to use.
const schema = BlockNoteSchema.create({
  blockSpecs: {
    // Adds all default blocks.
    ...defaultBlockSpecs,

    // Adds the code block.
    run: RunBlock,
  },
});

// Slash menu item to insert an Alert block
const insertRun = (editor: typeof schema.BlockNoteEditor) => ({
  title: "Code block",
  onItemClick: () => {
    insertOrUpdateBlock(editor, {
      type: "run",
    });
  },
  icon: <Code size={18} />,
  aliases: ["code", "run"],
  group: "Code",
});

export default function Editor() {
  // Creates a new editor instance.
  const editor = useCreateBlockNote({
    schema,
    initialContent: [
      {
        type: "heading",
        content: "Atuin runbooks",
        id: "foo",
      },
      {
        type: "run",
        id: "bar",
      },
    ],
  });

  // Renders the editor instance.
  return (
    <div>
      <BlockNoteView editor={editor} slashMenu={false} sideMenu={false}>
        <SuggestionMenuController
          triggerCharacter={"/"}
          getItems={async (query) =>
            filterSuggestionItems(
              [...getDefaultReactSlashMenuItems(editor), insertRun(editor)],
              query,
            )
          }
        />

        <SideMenuController
          sideMenu={(props) => (
            <SideMenu {...props}>
              <AddBlockButton {...props} />
              <DeleteBlock {...props} />
            </SideMenu>
          )}
        />
      </BlockNoteView>
    </div>
  );
}
