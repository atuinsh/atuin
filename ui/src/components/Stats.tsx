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

export default function Stats(props) {
  return (
    <div className="flex flex-col">
      <div className="flexfull">
        <dl className="grid grid-cols-1 sm:grid-cols-4 w-full">
          {props.stats.map((item) => (
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
