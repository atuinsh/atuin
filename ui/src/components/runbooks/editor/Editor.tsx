import { useEffect, useMemo, useState } from "react";

import "@blocknote/core/fonts/inter.css";
import "@blocknote/mantine/style.css";
import "./index.css";

import {
  BlockNoteSchema,
  BlockNoteEditor,
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
import { useDebounceCallback } from "usehooks-ts";

import RunBlock from "@/components/runbooks/editor/blocks/RunBlock";
import { DeleteBlock } from "@/components/runbooks/editor/ui/DeleteBlockButton";
import { useStore } from "@/state/store";
import Runbook from "@/state/runbooks/runbook";

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
  const runbookId = useStore((store) => store.currentRunbook);
  const refreshRunbooks = useStore((store) => store.refreshRunbooks);
  let [runbook, setRunbook] = useState<Runbook | null>(null);

  useEffect(() => {
    if (!runbookId) return;

    const fetchRunbook = async () => {
      let rb = await Runbook.load(runbookId);

      setRunbook(rb);
    };

    fetchRunbook();
  }, [runbookId]);

  const editor = useMemo(() => {
    if (!runbook) {
      return undefined;
    }

    if (runbook.content) {
      return BlockNoteEditor.create({
        initialContent: JSON.parse(runbook.content),
        schema,
      });
    }

    return BlockNoteEditor.create({ schema });
  }, [runbook]);

  const onChange = async () => {
    if (!runbook) return;

    console.log("saved!");
    runbook.name = fetchName();
    runbook.content = JSON.stringify(editor.document);

    await runbook.save();
    await refreshRunbooks();
  };

  const debouncedOnChange = useDebounceCallback(onChange, 1000);

  const fetchName = (): String => {
    // Infer the title from the first text block

    let blocks = editor.document;
    for (const block of blocks) {
      if (block.type == "heading" || block.type == "paragraph") {
        if (block.content.length == 0) continue;
        if (block.content[0].text.length == 0) continue;

        return block.content[0].text;
      }
    }

    return "Untitled";
  };

  if (editor === undefined) {
    return "Loading content...";
  }

  // Renders the editor instance.
  return (
    <div className="overflow-y-scroll w-full">
      <BlockNoteView
        editor={editor}
        slashMenu={false}
        sideMenu={false}
        onChange={debouncedOnChange}
      >
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
