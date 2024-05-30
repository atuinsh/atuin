import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import PacmanLoader from "react-spinners/PacmanLoader";

import {
  BarChart,
  Bar,
  XAxis,
  YAxis,
  Tooltip,
  ResponsiveContainer,
} from "recharts";

function renderLoading() {
  return (
    <div className="flex flex-col items-center justify-center h-full ">
      <div>
        <PacmanLoader color="#26bd65" />
      </div>
      <div className="block mt-4">
        <p>Crunching the latest numbers...</p>
      </div>
    </div>
  );
}

function TopTable({ stats }: any) {
  return (
    <div className="px-4 sm:px-6 lg:px-8">
      <div className="flex items-center">
        <div className="flex-auto">
          <h1 className="text-base font-semibold">Top commands</h1>
        </div>
      </div>
      <div className="mt-4 flow-root">
        <div className="-mx-4 -my-2 overflow-x-auto sm:-mx-6 lg:-mx-8">
          <div className="inline-block min-w-full py-2 align-middle">
            <table className="min-w-full divide-y divide-gray-300">
              <thead>
                <tr>
                  <th
                    scope="col"
                    className="py-3.5 pl-4 pr-3 text-left text-sm font-semibold text-gray-900 sm:pl-6 lg:pl-8"
                  >
                    Command
                  </th>
                  <th
                    scope="col"
                    className="px-3 py-3.5 text-left text-sm font-semibold text-gray-900"
                  >
                    Count
                  </th>
                </tr>
              </thead>
              <tbody className="divide-y divide-gray-200 bg-white">
                {stats.map((stat: any) => (
                  <tr>
                    <td className="whitespace-nowrap py-4 pl-4 pr-3 text-sm font-medium text-gray-900 sm:pl-6 lg:pl-8">
                      {stat[0][0]}
                    </td>
                    <td className="whitespace-nowrap px-3 py-4 text-sm text-gray-500">
                      {stat[1]}
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

export default function Stats() {
  const [stats, setStats]: any = useState([]);
  const [top, setTop]: any = useState([]);
  const [chart, setChart]: any = useState([]);

  useEffect(() => {
    if (stats.length != 0) return;

    invoke("global_stats")
      .then((s: any) => {
        console.log(s.daily);

        setStats([
          {
            name: "Total history",
            stat: s.total_history.toLocaleString(),
          },
          {
            name: "Unique history",
            stat: s.stats.unique_commands.toLocaleString(),
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

        setTop(s.stats);
      })
      .catch((e) => {
        console.log(e);
      });
  }, []);

  if (stats.length == 0) {
    return renderLoading();
  }

  return (
    <div className="flex flex-col overflow-y-scroll">
      <div className="flexfull">
        <dl className="grid grid-cols-1 sm:grid-cols-5 w-full">
          {stats.map((item: any) => (
            <div
              key={item.name}
              className="overflow-hidden bg-white px-4 py-5 shadow sm:p-6"
            >
              <dt className="truncate text-sm font-medium text-gray-500">
                {item.name}
              </dt>
              <dd className="mt-1 text-3xl font-semibold tracking-tight text-gray-900">
                {item.stat}
              </dd>
            </div>
          ))}
        </dl>
      </div>

      <div className="flex flex-col h-54 py-4 pl-5">
        <div className="flex flex-col h-48 pt-5 pr-5">
          <ResponsiveContainer width="100%" height="100%">
            <BarChart width={500} height={300} data={chart}>
              <XAxis dataKey="name" hide={true} />
              <YAxis />
              <Tooltip />
              <Bar dataKey="value" fill="#26bd65" />
            </BarChart>
          </ResponsiveContainer>
        </div>
      </div>

      <div>
        <TopTable stats={top.top} />
      </div>
    </div>
  );
}
