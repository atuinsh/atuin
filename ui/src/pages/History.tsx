import { useEffect, useRef } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";

import HistoryList from "@/components/HistoryList.tsx";
import HistorySearch from "@/components/HistorySearch.tsx";
import Stats from "@/components/history/Stats.tsx";
import Drawer from "@/components/Drawer.tsx";
import InfiniteHistory from "@/components/InfiniteHistory.tsx";

import { useStore } from "@/state/store";

import { inspectHistory, listHistory } from "@/state/models";
import { invoke } from "@tauri-apps/api/core";

function Header() {
  return (
    <div className="md:flex md:items-center md:justify-between">
      <div className="min-w-0 flex-1">
        <h2 className="text-2xl font-bold leading-7 text-gray-900 sm:truncate sm:text-3xl sm:tracking-tight">
          Shell History
        </h2>
      </div>
      <div className="flex">
        <Drawer
          width="70%"
          trigger={
            <button
              type="button"
              className="inline-flex border-2 items-center hover:shadow-xl rounded-md px-2 py-2 text-sm font-semibold shadow-sm"
            >
              <svg
                xmlns="http://www.w3.org/2000/svg"
                fill="none"
                viewBox="0 0 24 24"
                strokeWidth={1.5}
                stroke="currentColor"
                className="w-6 h-6"
              >
                <path
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  d="M3 13.125C3 12.504 3.504 12 4.125 12h2.25c.621 0 1.125.504 1.125 1.125v6.75C7.5 20.496 6.996 21 6.375 21h-2.25A1.125 1.125 0 0 1 3 19.875v-6.75ZM9.75 8.625c0-.621.504-1.125 1.125-1.125h2.25c.621 0 1.125.504 1.125 1.125v11.25c0 .621-.504 1.125-1.125 1.125h-2.25a1.125 1.125 0 0 1-1.125-1.125V8.625ZM16.5 4.125c0-.621.504-1.125 1.125-1.125h2.25C20.496 3 21 3.504 21 4.125v15.75c0 .621-.504 1.125-1.125 1.125h-2.25a1.125 1.125 0 0 1-1.125-1.125V4.125Z"
                />
              </svg>
            </button>
          }
        >
          <Stats />
        </Drawer>
      </div>
    </div>
  );
}

export default function Search() {
  const history = useStore((state) => state.shellHistory);
  const refreshHistory = useStore((state) => state.refreshShellHistory);

  useEffect(() => {
    (async () => {
      // nothing rn
    })();

    refreshHistory();
  }, []);

  const parentRef = useRef();

  const rowVirtualizer = useVirtualizer({
    count: history.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 90,
    overscan: 5,
  });

  return (
    <>
      <div className="pl-60">
        <div className="p-10 history-header">
          <Header />
          <p>A history of all the commands you run in your shell.</p>
        </div>

        <div className="flex h-16 shrink-0 items-center gap-x-4 border-b border-t border-gray-200 bg-white px-4 shadow-sm sm:gap-x-6 sm:px-6 lg:px-8 history-search">
          <HistorySearch
            refresh={(query?: string) => {
              refreshHistory(query);
            }}
          />
        </div>

        <main className="overflow-y-scroll history-list" ref={parentRef}>
          <HistoryList
            history={history}
            items={rowVirtualizer.getVirtualItems()}
            height={rowVirtualizer.getTotalSize()}
          />
        </main>
      </div>
    </>
  );
}
