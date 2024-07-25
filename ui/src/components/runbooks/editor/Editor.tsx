import { useEffect, useMemo, useState } from "react";

import "./index.css";

import { Spinner } from "@nextui-org/react";

// Errors, but it all works fine and is there. Maybe missing ts defs?
// I'll figure it out later
import {
  // @ts-ignore
  BlockNoteSchema,
  // @ts-ignore
  BlockNoteEditor,
  // @ts-ignore
  defaultBlockSpecs,
  // @ts-ignore
  filterSuggestionItems,
  // @ts-ignore
  insertOrUpdateBlock,
} from "@blocknote/core";

import {
  //@ts-ignore
  SuggestionMenuController,
  // @ts-ignore
  AddBlockButton,
  // @ts-ignore
  getDefaultReactSlashMenuItems,
  // @ts-ignore
  SideMenu,
  // @ts-ignore
  SideMenuController,
} from "@blocknote/react";
import { BlockNoteView } from "@blocknote/mantine";

import "@blocknote/core/fonts/inter.css";
import "@blocknote/mantine/style.css";

import { CodeIcon, FolderOpenIcon } from "lucide-react";
import { useDebounceCallback } from "usehooks-ts";

import Run from "@/components/runbooks/editor/blocks/Run";
import Directory from "@/components/runbooks/editor/blocks/Directory";

import { DeleteBlock } from "@/components/runbooks/editor/ui/DeleteBlockButton";
import { AtuinState, useStore } from "@/state/store";
import Runbook from "@/state/runbooks/runbook";

// Our schema with block specs, which contain the configs and implementations for blocks
// that we want our editor to use.
const schema = BlockNoteSchema.create({
  blockSpecs: {
    // Adds all default blocks.
    ...defaultBlockSpecs,

    // Adds the code block.
    run: Run,
    directory: Directory,
  },
});

// Slash menu item to insert an Alert block
const insertRun = (editor: typeof schema.BlockNoteEditor) => ({
  title: "Code",
  onItemClick: () => {
    insertOrUpdateBlock(editor, {
      type: "run",
    });
  },
  icon: <CodeIcon size={18} />,
  aliases: ["code", "run"],
  group: "Execute",
});

const insertDirectory = (editor: typeof schema.BlockNoteEditor) => ({
  title: "Directory",
  onItemClick: () => {
    insertOrUpdateBlock(editor, {
      type: "directory",
    });
  },
  icon: <FolderOpenIcon size={18} />,
  aliases: ["directory", "dir", "folder"],
  group: "Execute",
});

export default function Editor() {
  const runbookId = useStore((store: AtuinState) => store.currentRunbook);
  const refreshRunbooks = useStore(
    (store: AtuinState) => store.refreshRunbooks,
  );
  let [runbook, setRunbook] = useState<Runbook | null>(null);

  useEffect(() => {
    if (!runbookId) return;

    const fetchRunbook = async () => {
      let rb = await Runbook.load(runbookId);

      setRunbook(rb);
    };

    fetchRunbook();
  }, [runbookId]);

  const onChange = async () => {
    if (!runbook) return;

    console.log("saved!");
    runbook.name = fetchName();
    if (editor) runbook.content = JSON.stringify(editor.document);

    await runbook.save();
    refreshRunbooks();
  };

  const debouncedOnChange = useDebounceCallback(onChange, 1000);

  const editor = useMemo(() => {
    if (!runbook) return undefined;
    if (runbook.content) {
      return BlockNoteEditor.create({
        initialContent: JSON.parse(runbook.content),
        schema,
      });
    }

    return BlockNoteEditor.create({ schema });
  }, [runbook]);

  const fetchName = (): string => {
    // Infer the title from the first text block
    if (!editor) return "Untitled";

    let blocks = editor.document;
    for (const block of blocks) {
      if (block.type == "heading" || block.type == "paragraph") {
        if (block.content.length == 0) continue;
        // @ts-ignore
        if (block.content[0].text.length == 0) continue;

        // @ts-ignore
        return block.content[0].text;
      }
    }

    return "Untitled";
  };

  if (!runbook) {
    return (
      <div className="flex w-full h-full flex-col justify-center items-center">
        <Spinner />
      </div>
    );
  }

  if (editor === undefined) {
    return (
      <div className="flex w-full h-full flex-col justify-center items-center">
        <Spinner />
      </div>
    );
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
          getItems={async (query: any) =>
            filterSuggestionItems(
              [
                ...getDefaultReactSlashMenuItems(editor),
                insertRun(editor),
                insertDirectory(editor),
              ],
              query,
            )
          }
        />

        <SideMenuController
          sideMenu={(props: any) => (
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
