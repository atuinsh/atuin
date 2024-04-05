import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  BarChart,
  Bar,
  Rectangle,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  Legend,
  ResponsiveContainer,
} from "recharts";

function classNames(...classes) {
  return classes.filter(Boolean).join(" ");
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

export default function Stats() {
  const [stats, setStats]: any = useState([]);
  const [chart, setChart]: any = useState([]);

  useEffect(() => {
    if (stats.length != 0) return;

    loadStats(setStats, setChart);
  }, []);

  return (
    <div className="flex flex-col">
      <div className="flexfull">
        <dl className="grid grid-cols-1 sm:grid-cols-4 w-full">
          {stats.map((item) => (
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
    </div>
  );
}
