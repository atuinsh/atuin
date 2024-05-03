import { useState } from "react";
import { ArrowPathIcon } from "@heroicons/react/24/outline";
import { MagnifyingGlassIcon } from "@heroicons/react/20/solid";

interface HistorySearchProps {
  query: string;
  refresh: () => void;
  setQuery: (query: string) => void;
}

export default function HistorySearch(props: HistorySearchProps) {
  return (
    <div className="flex flex-1 gap-x-4 self-stretch lg:gap-x-6">
      <form
        className="relative flex flex-1"
        onSubmit={(e) => {
          e.preventDefault();
        }}
      >
        <label htmlFor="search-field" className="sr-only">
          Search
        </label>
        <MagnifyingGlassIcon
          className="pointer-events-none absolute inset-y-0 left-0 h-full w-5 text-gray-400"
          aria-hidden="true"
        />
        <input
          id="search-field"
          className="block h-full w-full border-0 py-0 pl-8 pr-0 text-gray-900 placeholder:text-gray-400 focus:ring-0 sm:text-sm"
          placeholder="Search..."
          autoComplete="off"
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck="false"
          type="search"
          name="search"
          onChange={(query) => {
            props.setQuery(query.target.value);
            props.refresh();
          }}
        />
      </form>
      <div className="flex items-center gap-x-4 lg:gap-x-6">
        <button
          type="button"
          className="-m-2.5 p-2.5 text-gray-400 hover:text-gray-500"
          onClick={() => {
            props.refresh();
          }}
        >
          <ArrowPathIcon className="h-6 w-6" aria-hidden="true" />
        </button>
      </div>
    </div>
  );
}
