import { useRef } from "react";
import { ChevronRightIcon } from "@heroicons/react/20/solid";

// @ts-ignore
import { DateTime } from "luxon";

function msToTime(ms: number) {
  let milliseconds = parseInt(ms.toFixed(1));
  let seconds = parseInt((ms / 1000).toFixed(1));
  let minutes = parseInt((ms / (1000 * 60)).toFixed(1));
  let hours = parseInt((ms / (1000 * 60 * 60)).toFixed(1));
  let days = parseInt((ms / (1000 * 60 * 60 * 24)).toFixed(1));

  if (milliseconds < 1000) return milliseconds + "ms";
  else if (seconds < 60) return seconds + "s";
  else if (minutes < 60) return minutes + "m";
  else if (hours < 24) return hours + "hr";
  else return days + " Days";
}

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
          <li
            key={h.id}
            className="relative flex justify-between gap-x-6 px-4 py-5 hover:bg-gray-50 sm:px-6"
            style={{
              position: "absolute",
              top: 0,
              left: 0,
              width: "100%",
              height: `${i.size}px`,
              transform: `translateY(${i.start}px)`,
            }}
          >
            <div className="flex min-w-0 gap-x-4">
              <div className="flex flex-col justify-center">
                <p className="flex text-xs text-gray-500 justify-center">
                  {DateTime.fromMillis(h.timestamp / 1000000).toLocaleString(
                    DateTime.TIME_WITH_SECONDS,
                  )}
                </p>
                <p className="flex text-xs mt-1 text-gray-400 justify-center">
                  {DateTime.fromMillis(h.timestamp / 1000000).toLocaleString(
                    DateTime.DATE_SHORT,
                  )}
                </p>
              </div>
              <div className="min-w-0 flex-col justify-center">
                <pre className="whitespace-pre-wrap">
                  <code className="text-sm">{h.command}</code>
                </pre>
                <p className="mt-1 flex text-xs leading-5 text-gray-500">
                  <span className="relative truncate ">{h.user}</span>

                  <span>&nbsp;on&nbsp;</span>

                  <span className="relative truncate ">{h.host}</span>

                  <span>&nbsp;in&nbsp;</span>

                  <span className="relative truncate ">{h.cwd}</span>
                </p>
              </div>
            </div>
            <div className="flex shrink-0 items-center gap-x-4">
              <div className="hidden sm:flex sm:flex-col sm:items-end">
                <p className="text-sm leading-6 text-gray-900">{h.exit}</p>
                {h.duration ? (
                  <p className="mt-1 text-xs leading-5 text-gray-500">
                    <time dateTime={h.duration}>
                      {msToTime(h.duration / 1000000)}
                    </time>
                  </p>
                ) : (
                  <div />
                )}
              </div>
              <ChevronRightIcon
                className="h-5 w-5 flex-none text-gray-400"
                aria-hidden="true"
              />
            </div>
          </li>
        );
      })}
    </div>
  );
}
