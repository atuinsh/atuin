import { useEffect, useState, useRef } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";

import HistoryList from "@/components/HistoryList.tsx";
import HistorySearch from "@/components/HistorySearch.tsx";

import { AtuinState, useStore } from "@/state/store";

export default function Search() {
  const history = useStore((state: AtuinState) => state.shellHistory);
  const refreshHistory = useStore(
    (state: AtuinState) => state.refreshShellHistory,
  );
  const historyNextPage = useStore(
    (state: AtuinState) => state.historyNextPage,
  );

  let [query, setQuery] = useState("");

  useEffect(() => {
    (async () => {
      // nothing rn
    })();

    refreshHistory();
  }, []);

  const parentRef = useRef<HTMLElement | null>(null);

  const rowVirtualizer = useVirtualizer({
    count: history.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 90,
    overscan: 5,
  });

  useEffect(() => {
    const [lastItem] = rowVirtualizer.getVirtualItems().slice(-1);

    if (!lastItem) return; // no undefined plz
    if (lastItem.index < history.length - 1) return; // if we're not at the end yet, bail

    // we're at the end! more rows plz!
    historyNextPage(query);
  }, [rowVirtualizer.getVirtualItems()]);

  return (
    <>
      <div className="w-full flex-1 flex-col">
        <div className="flex h-16 shrink-0 items-center gap-x-4 border-b border-t border-gray-200 bg-white px-4 shadow-sm sm:gap-x-6 sm:px-6 lg:px-8 history-search">
          <HistorySearch
            query={query}
            setQuery={(q) => {
              setQuery(q);
              refreshHistory(q);
            }}
            refresh={() => {
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
