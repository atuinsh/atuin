import { useEffect, useState } from "react";
import {
  Input,
  Button,
  ButtonGroup,
  Card,
  CardBody,
  CardHeader,
  Divider,
  Tooltip,
  Listbox,
  ListboxItem,
  Dropdown,
  DropdownTrigger,
  DropdownMenu,
  DropdownItem,
} from "@nextui-org/react";

import { EllipsisVerticalIcon } from "lucide-react";

import { DateTime } from "luxon";

import { NotebookPenIcon } from "lucide-react";
import Runbook from "@/state/runbooks/runbook";
import { useStore } from "@/state/store";

const NoteSidebar = () => {
  const runbooks = useStore((state) => state.runbooks);
  const refreshRunbooks = useStore((state) => state.refreshRunbooks);

  const currentRunbook = useStore((state) => state.currentRunbook);
  const setCurrentRunbook = useStore((state) => state.setCurrentRunbook);

  useEffect(() => {
    refreshRunbooks();
  }, []);

  return (
    <div className="w-48 flex flex-col border-r-1">
      <div className="overflow-y-auto flex-grow">
        <Listbox
          hideSelectedIcon
          items={runbooks}
          variant="flat"
          aria-label="Runbook list"
          selectionMode="single"
          selectedKeys={[currentRunbook]}
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
          {(runbook) => (
            <ListboxItem
              key={runbook.id}
              onPress={() => {
                setCurrentRunbook(runbook.id);
              }}
              textValue={runbook.name || "Untitled"}
              endContent={
                <Dropdown>
                  <DropdownTrigger className="bg-transparent">
                    <Button isIconOnly>
                      <EllipsisVerticalIcon
                        size="16px"
                        className="bg-transparent"
                      />
                    </Button>
                  </DropdownTrigger>
                  <DropdownMenu aria-label="Dynamic Actions">
                    <DropdownItem
                      key={"delete"}
                      color="danger"
                      className="text-danger"
                      onPress={async () => {
                        await Runbook.delete(runbook.id);
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
                      DateTime.DATETIME_SIMPLE,
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
