import { useState, useEffect } from 'react'
import { invoke } from '@tauri-apps/api/core';
import { BarChart, Bar, Rectangle, XAxis, YAxis, CartesianGrid, Tooltip, Legend, ResponsiveContainer } from 'recharts';

const tabs = [
  { name: 'Daily', href: '#', current: true },
  { name: 'Weekly', href: '#', current: false },
  { name: 'Monthly', href: '#', current: false },
]

function classNames(...classes) {
  return classes.filter(Boolean).join(' ')
}

export default function Stats() {
  const [stats, setStats]: any = useState([]);
  const [chart, setChart]: any = useState([]);

  useEffect(() => {
    if (stats.length != 0) return;

    invoke("global_stats").then((s: any) => {
      console.log(s.daily);

      setStats([
        {
          name: "Total history",
          stat: s.total_history.toLocaleString()
        },
        {
          name: "Last 1d",
          stat: s.last_1d.toLocaleString()
        },
        {
          name: "Last 7d",
          stat: s.last_7d.toLocaleString()
        },
        {
          name: "Last 30d",
          stat: s.last_30d.toLocaleString()
        },
      ]);

      setChart(s.daily);
    }).catch((e) => {
      console.log(e);
    });
  }, []);

  return (
    <div className="lg:pl-60 flex flex-col">

      <div className="flexfull">
        <dl className="grid grid-cols-1 sm:grid-cols-4 w-full">
          {stats.map((item) => (
            <div key={item.name} className="overflow-hidden bg-white px-4 py-5 shadow sm:p-6">
              <dt className="truncate text-sm font-medium text-gray-500">{item.name}</dt>
              <dd className="mt-1 text-3xl font-semibold tracking-tight text-gray-900">{item.stat}</dd>
            </div>
          ))}
        </dl>
      </div>

      <div className="flex flex-col h-54 py-4 pl-5">
        <div className="sm:hidden">
          {/* Use an "onChange" listener to redirect the user to the selected tab URL. */}
          <select
            id="tabs"
            name="tabs"
            className="block w-full rounded-md border-gray-300 focus:border-indigo-500 focus:ring-indigo-500"
            defaultValue={tabs.find((tab) => tab.current).name}
          >
            {tabs.map((tab) => (
              <option key={tab.name}>{tab.name}</option>
            ))}
          </select>
        </div>
        <div className="hidden sm:block">
          <nav className="flex space-x-4" aria-label="Tabs">
            {tabs.map((tab) => (
              <a
                key={tab.name}
                href={tab.href}
                className={classNames(
                  tab.current ? 'bg-gray-100 text-gray-700' : 'text-gray-500 hover:text-gray-700',
                  'rounded-md px-3 py-2 text-sm font-medium'
                )}
                aria-current={tab.current ? 'page' : undefined}
              >
                {tab.name}
              </a>
            ))}
          </nav>
        </div>

        <div className="flex flex-col h-48 pt-5 pr-5">
          <ResponsiveContainer width="100%" height="100%">
            <BarChart
              width={500}
              height={300}
              data={chart}
            >
              <XAxis dataKey="name" hide={true} />
              <YAxis />
              <Tooltip />
              <Bar dataKey="value"fill="#26bd65"  />
            </BarChart>
          </ResponsiveContainer>
        </div>
      </div>
    </div>
  );
}


