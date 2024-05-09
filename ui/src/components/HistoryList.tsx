import { useRef } from "react";
import HistoryRow from "./history/HistoryRow";

export default function HistoryList(props: any) {
  return (
    <div
      role="list"
      className="divide-y divide-gray-100 bg-white shadow-sm ring-1 ring-gray-900/5 overflow-auto"
      style={{
        height: `${props.height}px`,
        position: "relative",
      }}
    >
      {props.items.map((i: any) => {
        let h = props.history[i.index];

        return <HistoryRow h={h} size={i.size} start={i.start} />;
      })}
    </div>
  );
}
