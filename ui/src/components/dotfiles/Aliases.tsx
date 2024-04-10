import React, { useEffect, useState } from "react";

import DataTable from "@/components/ui/data-table";
import { Button } from "@/components/ui/button";
import { MoreHorizontal } from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

import { invoke } from "@tauri-apps/api/core";

function loadAliases(setAliases: React.Dispatch<React.SetStateAction<any[]>>) {
  invoke("aliases").then((aliases: any) => {
    setAliases(aliases);
  });
}

type Alias = {
  name: string;
  value: string;
};

function deleteAlias(name: string) {
  invoke("delete_alias", { name: name })
    .then(() => {
      console.log("Deleted alias");
    })
    .catch(() => {
      console.error("Failed to delete alias");
    });
}

const columns: ColumnDef<Alias>[] = [
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
    cell: ({ row }) => {
      const alias = row.original;

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
            <DropdownMenuItem onClick={() => deleteAlias(alias.name)}>
              Delete
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      );
    },
  },
];

export default function Aliases() {
  let [aliases, setAliases] = useState([]);

  useEffect(() => {
    loadAliases(setAliases);
  }, []);

  return (
    <div className="pt-10">
      <div className="sm:flex sm:items-center">
        <div className="sm:flex-auto">
          <h1 className="text-base font-semibold leading-6 text-gray-900">
            Aliases
          </h1>
          <p className="mt-2 text-sm text-gray-700">
            Aliases allow you to condense long commands into short,
            easy-to-remember commands.
          </p>
        </div>
        <div className="mt-4 sm:ml-16 sm:mt-0 flex-row">
          <button
            type="button"
            className="block rounded-md bg-green-600 px-3 py-2 text-center text-sm font-semibold text-white shadow-sm hover:bg-green-500 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-green-600"
          >
            Add
          </button>
        </div>
      </div>
      <div className="mt-8 flow-root">
        <div className="-mx-4 -my-2 overflow-x-auto sm:-mx-6 lg:-mx-8">
          <div className="inline-block min-w-full py-2 align-middle sm:px-6 lg:px-8">
            <DataTable columns={columns} data={aliases} />
          </div>
        </div>
      </div>
    </div>
  );
}
