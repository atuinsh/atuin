import { Cog6ToothIcon } from "@heroicons/react/24/outline";

import Aliases from "@/components/dotfiles/Aliases";

function Header() {
  return (
    <div className="md:flex md:items-center md:justify-between">
      <div className="min-w-0 flex-1">
        <h2 className="text-2xl font-bold leading-7 text-gray-900 sm:truncate sm:text-3xl sm:tracking-tight">
          Dotfiles
        </h2>
      </div>
      <div className="mt-4 flex md:ml-4 md:mt-0">
        <button
          type="button"
          className="inline-flex items-center rounded-md bg-indigo-600 px-3 py-2 text-sm font-semibold text-white shadow-sm hover:bg-indigo-700 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-indigo-600"
        >
          Import
        </button>
      </div>
    </div>
  );
}

export default function Dotfiles() {
  return (
    <div className="pl-60">
      <div className="p-10">
        <Header />
        Manage your shell aliases, variables and paths
        <Aliases />
      </div>
    </div>
  );
}
