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
    <div className="flex items-center justify-center h-full">
      <PacmanLoader color="#26bd65" />
    </div>
  );
}

export default function Stats() {
  const [stats, setStats]: any = useState([]);
  const [chart, setChart]: any = useState([]);

  console.log("Stats mounted");

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
  }, []);

  if (stats.length == 0) {
    return renderLoading();
  }

  return (
    <div className="flex flex-col">
      <div className="flexfull">
        <dl className="grid grid-cols-1 sm:grid-cols-4 w-full">
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
    </div>
  );
}
