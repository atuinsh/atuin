// @ts-ignore
import { DateTime } from "luxon";
import { ChevronRightIcon } from "@heroicons/react/20/solid";
import { Highlight, themes } from "prism-react-renderer";

// @ts-ignore
import Prism from "prismjs";

// @ts-ignore
import "prismjs/components/prism-bash";

import Drawer from "../Drawer";
import HistoryInspect from "./HistoryInspect";
import { cn } from "@/lib/utils";

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

export default function HistoryRow({ h, compact }: any) {
  return (
    <li
      key={h.id}
      className={cn(
        "relative flex justify-between gap-x-6 px-4 py-5 hover:bg-gray-50 sm:px-6",
        { "py-5": !compact },
        { "py-1": compact },
      )}
    >
      <div className="flex min-w-0 gap-x-4">
        {!compact && (
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
        )}
        <div className="min-w-0 flex-col justify-center truncate">
          <Highlight
            theme={themes.github}
            code={h.command}
            language="bash"
            prism={Prism}
          >
            {({ style, tokens, getLineProps, getTokenProps }) => (
              <pre style={style} className="!bg-inherit text-sm">
                {tokens &&
                  tokens.map((line, i) => {
                    if (i != 0) return;
                    return (
                      <div key={i} {...getLineProps({ line })}>
                        {line.map((token, key) => (
                          <span key={key} {...getTokenProps({ token })} />
                        ))}
                      </div>
                    );
                  })}
              </pre>
            )}
          </Highlight>
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
        <Drawer
          width="60%"
          trigger={
            <button type="button">
              <ChevronRightIcon
                className="h-5 w-5 flex-none text-gray-400"
                aria-hidden="true"
              />
            </button>
          }
        >
          <HistoryInspect history={h} />
        </Drawer>
      </div>
    </li>
  );
}
