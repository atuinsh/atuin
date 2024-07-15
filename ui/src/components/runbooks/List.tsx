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
} from "@nextui-org/react";
import { DateTime } from "luxon";

import { NotebookPenIcon } from "lucide-react";
import Runbook from "@/state/runbooks/runbook";
import { useStore } from "@/state/store";

const NoteSidebar = () => {
  const runbooks = useStore((state) => state.runbooks);
  const refreshRunbooks = useStore((state) => state.refreshRunbooks);

  useEffect(() => {
    refreshRunbooks();
  }, []);

  return (
    <div className="min-w-48 h-screen flex flex-col border-r-1">
      <div className="flex flex-row">
        <ButtonGroup>
          <Tooltip showArrow content="New Runbook" closeDelay={50}>
            <Button
              isIconOnly
              aria-label="New note"
              variant="light"
              size="sm"
              onPress={async () => {
                console.log(await Runbook.all());
              }}
            >
              <NotebookPenIcon className="p-[0.15rem]" />
            </Button>
          </Tooltip>
        </ButtonGroup>
      </div>

      <div className="overflow-y-auto flex-grow">
        {runbooks.map((runbook) => (
          <Card
            key={runbook.id.toString()}
            className="cursor-pointer hover:bg-gray-200"
            radius="sm"
            shadow="none"
          >
            <CardBody className="px-3 flex flex-col">
              <h1 className="text-md">{runbook.name}</h1>

              <div className="flex flex-row">
                <div className="text-xs flex-1">
                  {runbook.updated.toLocaleString()}
                </div>
              </div>
            </CardBody>
          </Card>
        ))}
      </div>
    </div>
  );
};

export default NoteSidebar;
