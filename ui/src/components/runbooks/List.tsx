import { useEffect } from "react";
import {
  Button,
  ButtonGroup,
  Tooltip,
  Listbox,
  ListboxItem,
  Dropdown,
  DropdownTrigger,
  DropdownMenu,
  DropdownItem,
  Badge,
} from "@nextui-org/react";

import { EllipsisVerticalIcon } from "lucide-react";

import { DateTime } from "luxon";

import { NotebookPenIcon } from "lucide-react";
import Runbook from "@/state/runbooks/runbook";
import { AtuinState, useStore } from "@/state/store";

const NoteSidebar = () => {
  const runbooks = useStore((state: AtuinState) => state.runbooks);
  const refreshRunbooks = useStore(
    (state: AtuinState) => state.refreshRunbooks,
  );

  const currentRunbook = useStore((state: AtuinState) => state.currentRunbook);
  const setCurrentRunbook = useStore(
    (state: AtuinState) => state.setCurrentRunbook,
  );
  const runbookInfo = useStore((state: AtuinState) => state.runbookInfo);

  useEffect(() => {
    refreshRunbooks();
  }, []);

  return (
    <div className="w-48 flex flex-col border-r-1">
      <div className="overflow-y-auto flex-grow">
        <Listbox
          hideSelectedIcon
          items={runbooks.map((runbook: any): any => {
            return [runbook, runbookInfo[runbook.id]];
          })}
          variant="flat"
          aria-label="Runbook list"
          selectionMode="single"
          selectedKeys={currentRunbook ? [currentRunbook] : []}
          itemClasses={{ base: "data-[selected=true]:bg-gray-200" }}
          topContent={
            <ButtonGroup className="z-20">
              <Tooltip showArrow content="New Runbook" closeDelay={50}>
                <Button
                  isIconOnly
                  aria-label="New note"
                  variant="light"
                  size="sm"
                  onPress={async () => {
                    // otherwise the cursor is weirdly positioned in the new document
                    window.getSelection()?.removeAllRanges();

                    let runbook = await Runbook.create();
                    setCurrentRunbook(runbook.id);
                    refreshRunbooks();
                  }}
                >
                  <NotebookPenIcon className="p-[0.15rem]" />
                </Button>
              </Tooltip>
            </ButtonGroup>
          }
        >
          {([runbook, info]: [Runbook, { ptys: number }]) => (
            <ListboxItem
              key={runbook.id}
              onPress={() => {
                setCurrentRunbook(runbook.id);
              }}
              textValue={runbook.name || "Untitled"}
              endContent={
                <Dropdown>
                  <Badge
                    content={info?.ptys}
                    color="primary"
                    style={
                      info && info?.ptys > 0
                        ? {}
                        : {
                            display: "none",
                          }
                    }
                  >
                    <DropdownTrigger className="bg-transparent">
                      <Button isIconOnly>
                        <EllipsisVerticalIcon
                          size="16px"
                          className="bg-transparent"
                        />
                      </Button>
                    </DropdownTrigger>
                  </Badge>
                  <DropdownMenu aria-label="Dynamic Actions">
                    <DropdownItem
                      key={"delete"}
                      color="danger"
                      className="text-danger"
                      onPress={async () => {
                        await Runbook.delete(runbook.id);

                        if (runbook.id == currentRunbook) setCurrentRunbook("");

                        refreshRunbooks();
                      }}
                    >
                      Delete
                    </DropdownItem>
                  </DropdownMenu>
                </Dropdown>
              }
            >
              <div className="flex flex-col">
                <div className="text-md">{runbook.name || "Untitled"}</div>
                <div className="text-xs text-gray-500">
                  <em>
                    {DateTime.fromJSDate(runbook.updated).toLocaleString(
                      DateTime.DATETIME_SHORT,
                    )}
                  </em>
                </div>
              </div>
            </ListboxItem>
          )}
        </Listbox>
      </div>
    </div>
  );
};

export default NoteSidebar;
