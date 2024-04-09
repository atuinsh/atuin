import React, { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

function loadAliases(setAliases: React.Dispatch<React.SetStateAction<any[]>>) {
  invoke("aliases").then((aliases: any) => {
    setAliases(aliases);
  });
}

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
            All configured shell aliases. Aliases allow you to condense long
            commands into short, easy-to-remember commands.
          </p>
        </div>
        <div className="mt-4 sm:ml-16 sm:mt-0 flex-row">
          <button
            type="button"
            className="block rounded-md bg-indigo-600 px-3 py-2 text-center text-sm font-semibold text-white shadow-sm hover:bg-indigo-500 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-indigo-600"
          >
            Add
          </button>
        </div>
      </div>
      <div className="mt-8 flow-root">
        <div className="-mx-4 -my-2 overflow-x-auto sm:-mx-6 lg:-mx-8">
          <div className="inline-block min-w-full py-2 align-middle sm:px-6 lg:px-8">
            <table className="min-w-full divide-y divide-gray-300">
              <thead>
                <tr className="divide-x divide-gray-200">
                  <th
                    scope="col"
                    className="py-3.5 pl-4 pr-4 text-left text-sm font-semibold text-gray-900 sm:pl-0"
                  >
                    Name
                  </th>
                  <th
                    scope="col"
                    className="px-4 py-3.5 text-left text-sm font-semibold text-gray-900"
                  >
                    Value
                  </th>
                </tr>
              </thead>
              <tbody className="divide-y divide-gray-200 bg-white">
                {aliases.map((person) => (
                  <tr key={person.name} className="divide-x divide-gray-200">
                    <td className="whitespace-nowrap py-4 pl-4 pr-4 text-sm font-medium text-gray-900 sm:pl-0">
                      {person.name}
                    </td>
                    <td className="whitespace-nowrap p-4 text-sm text-gray-500">
                      {person.value}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      </div>
    </div>
  );
}
