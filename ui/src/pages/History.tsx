import { Fragment, useState, useEffect } from "react";
import { Dialog, Transition } from "@headlessui/react";
import {
  Bars3Icon,
  ChartPieIcon,
  Cog6ToothIcon,
  HomeIcon,
  XMarkIcon,
} from "@heroicons/react/24/outline";
import Logo from "../assets/logo-light.svg";

import { invoke } from "@tauri-apps/api/core";

import HistoryList from "../components/HistoryList.tsx";
import HistorySearch from "../components/HistorySearch.tsx";
import Stats from "../components/Stats.tsx";

function refreshHistory(
  setHistory: React.Dispatch<React.SetStateAction<never[]>>,
  query: String | null,
) {
  if (query) {
    invoke("search", { query: query })
      .then((res: any[]) => {
        setHistory(res);
      })
      .catch((e) => {
        console.log(e);
      });
  } else {
    invoke("list").then((h: any[]) => {
      setHistory(h);
    });
  }
}

function loadStats(setStats, setChart) {
  invoke("global_stats")
    .then((s: any) => {
      console.log(s.daily);

      setStats([
        {
          name: "Total history",
          stat: s.total_history.toLocaleString(),
        },
        {
          name: "Last 1d",
          stat: s.last_1d.toLocaleString(),
        },
        {
          name: "Last 7d",
          stat: s.last_7d.toLocaleString(),
        },
        {
          name: "Last 30d",
          stat: s.last_30d.toLocaleString(),
        },
      ]);

      setChart(s.daily);
    })
    .catch((e) => {
      console.log(e);
    });
}

function Header() {
  return (
    <div className="md:flex md:items-center md:justify-between">
      <div className="min-w-0 flex-1">
        <h2 className="text-2xl font-bold leading-7 text-gray-900 sm:truncate sm:text-3xl sm:tracking-tight">
          Shell History
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

export default function Search() {
  let [history, setHistory] = useState([]);
  let [stats, setStats] = useState([]);
  let [chart, setChart] = useState(null);

  useEffect(() => {
    refreshHistory(setHistory, null);

    if (stats.length == 0) loadStats(setStats, setChart);
  }, []);

  return (
    <>
      <div className="pl-60">
        <div className="p-10">
          <Header />
          <p>A history of all the commands you run in your shell.</p>
        </div>

        <div className="flex h-16 shrink-0 items-center gap-x-4 border-b border-t border-gray-200 bg-white px-4 shadow-sm sm:gap-x-6 sm:px-6 lg:px-8">
          <HistorySearch
            refresh={(query: String | null) => {
              refreshHistory(setHistory, query);
              loadStats(setStats, setChart);
            }}
          />
        </div>

        <main w>
          <HistoryList history={history} />
        </main>
      </div>
    </>
  );
}
