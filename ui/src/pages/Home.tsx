import { Fragment, useState, useEffect } from 'react'
import { Dialog, Transition } from '@headlessui/react'
import {
  Bars3Icon,
  ChartPieIcon,
  Cog6ToothIcon,
  HomeIcon,
  XMarkIcon,
} from '@heroicons/react/24/outline'
import Logo from "../assets/logo-light.svg";

import { invoke } from '@tauri-apps/api/core';

import HistoryList from "../components/HistoryList.tsx";
import HistorySearch from "../components/HistorySearch.tsx";




export default function Home() {
  let [history, setHistory] = useState([]);

  useEffect(() => {
    invoke("list").then((h) =>{
      setHistory(h);
    });
  }, []);

  return (
    <>
        <div className="lg:pl-60">
          <div className="sticky top-0 z-40 flex h-16 shrink-0 items-center gap-x-4 border-b border-gray-200 bg-white px-4 shadow-sm sm:gap-x-6 sm:px-6 lg:px-8">
            <button type="button" className="-m-2.5 p-2.5 text-gray-700 lg:hidden" onClick={() => setSidebarOpen(true)}>
              <span className="sr-only">Open sidebar</span>
              <Bars3Icon className="h-6 w-6" aria-hidden="true" />
            </button>


            <div className="h-6 w-px bg-gray-200 lg:hidden" aria-hidden="true" />

            <HistorySearch setHistory={setHistory} />
          </div>

          <main w>
            <HistoryList history={history}/>
          </main>
        </div>
    </>
  )
}
