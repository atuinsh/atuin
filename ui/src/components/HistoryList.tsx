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

        return (
          <div
            style={{
              position: "absolute",
              top: 0,
              left: 0,
              width: "100%",
              height: `${i.size}px`,
              transform: `translateY(${i.start}px)`,
            }}
          >
            <HistoryRow h={h} />
          </div>
        );
      })}
    </div>
  );
}
