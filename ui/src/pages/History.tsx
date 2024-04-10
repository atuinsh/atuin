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

import HistoryList from "@/components/HistoryList.tsx";
import HistorySearch from "@/components/HistorySearch.tsx";
import Stats from "@/components/history/Stats.tsx";
import Drawer from "@/components/Drawer.tsx";

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

function Header() {
  return (
    <div className="md:flex md:items-center md:justify-between">
      <div className="min-w-0 flex-1">
        <h2 className="text-2xl font-bold leading-7 text-gray-900 sm:truncate sm:text-3xl sm:tracking-tight">
          Shell History
        </h2>
      </div>
      <div className="mt-4 flex md:ml-4 md:mt-0">
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
  let [history, setHistory] = useState([]);

  useEffect(() => {
    refreshHistory(setHistory, null);
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
            }}
          />
        </div>

        <main>
          <HistoryList history={history} />
        </main>
      </div>
    </>
  );
}
