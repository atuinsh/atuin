import { useEffect, useState } from "react";

import DataTable from "@/components/ui/data-table";
import { Button } from "@/components/ui/button";
import { MoreHorizontal } from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

import { ColumnDef } from "@tanstack/react-table";

import { invoke } from "@tauri-apps/api/core";
import Drawer from "@/components/Drawer";

import { Var } from "@/state/models";
import { useStore } from "@/state/store";

function deleteVar(name: string, refreshVars: () => void) {
  invoke("delete_var", { name: name })
    .then(() => {
      refreshVars();
    })
    .catch(() => {
      console.error("Failed to delete var");
    });
}

function AddVar({ onAdd: onAdd }: { onAdd?: () => void }) {
  let [name, setName] = useState("");
  let [value, setValue] = useState("");
  let [exp, setExport] = useState<boolean>(false);

  // simple form to add vars
  return (
    <div className="p-4">
      <h2 className="text-xl font-semibold leading-6 text-gray-900">Add var</h2>
      <p className="mt-2">Add a new var to your shell</p>

      <form
        className="mt-4"
        onSubmit={(e) => {
          e.preventDefault();

          invoke("set_var", { name: name, value: value, export: exp })
            .then(() => {
              console.log("Added var");

              if (onAdd) onAdd();
            })
            .catch(() => {
              console.error("Failed to add var");
            });
        }}
      >
        <input
          className="bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-md focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5"
          type="text"
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="Var name"
        />

        <input
          className="mt-4 bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-md focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5"
          autoComplete="off"
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck="false"
          type="text"
          value={value}
          onChange={(e) => setValue(e.target.value)}
          placeholder="Var value"
        />

        <div>
          <label>
            <input
              className="mt-4 bg-gray-50 mr-2 inline"
              autoComplete="off"
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck="false"
              type="checkbox"
              value={exp.toString()}
              onChange={(e) => setExport(e.target.checked)}
            />
            Export the var and make it visible to subprocesses
          </label>
        </div>

        <input
          type="submit"
          className="block mt-4 rounded-md bg-green-600 px-3 py-2 text-center text-sm font-semibold text-white shadow-sm hover:bg-green-500 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-green-600"
          value="Add var"
        />
      </form>
    </div>
  );
}

export default function Vars() {
  const vars = useStore((state) => state.vars);
  const refreshVars = useStore((state) => state.refreshVars);

  let [varDrawerOpen, setVarDrawerOpen] = useState(false);

  const columns: ColumnDef<Var>[] = [
    {
      accessorKey: "name",
      header: "Name",
    },
    {
      accessorKey: "value",
      header: "Value",
    },
    {
      id: "actions",
      cell: ({ row }: any) => {
        const shell_var = row.original;

        return (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" className="h-8 w-8 p-0 float-right">
                <span className="sr-only">Open menu</span>
                <MoreHorizontal className="h-4 w-4 text-right" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuLabel>Actions</DropdownMenuLabel>
              <DropdownMenuItem
                onClick={() => deleteVar(shell_var.name, refreshVars)}
              >
                Delete
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        );
      },
    },
  ];

  useEffect(() => {
    refreshVars();
  }, []);

  return (
    <div className="pt-10">
      <div className="sm:flex sm:items-center">
        <div className="sm:flex-auto">
          <h1 className="text-base font-semibold leading-6 text-gray-900">
            Vars
          </h1>
          <p className="mt-2 text-sm text-gray-700">
            Configure environment variables here
          </p>
        </div>
        <div className="mt-4 sm:ml-16 sm:mt-0 flex-row">
          <Drawer
            open={varDrawerOpen}
            onOpenChange={setVarDrawerOpen}
            width="30%"
            trigger={
              <button
                type="button"
                className="block rounded-md bg-green-600 px-3 py-2 text-center text-sm font-semibold text-white shadow-sm hover:bg-green-500 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-green-600"
              >
                Add
              </button>
            }
          >
            <AddVar
              onAdd={() => {
                refreshVars();
                setVarDrawerOpen(false);
              }}
            />
          </Drawer>
        </div>
      </div>
      <div className="mt-8 flow-root">
        <div className="-mx-4 -my-2 overflow-x-auto sm:-mx-6 lg:-mx-8">
          <div className="inline-block min-w-full py-2 align-middle sm:px-6 lg:px-8">
            <DataTable columns={columns} data={vars} />
          </div>
        </div>
      </div>
    </div>
  );
}
