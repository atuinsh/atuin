import { DateTime } from 'luxon';
import { ChevronRightIcon } from '@heroicons/react/20/solid'

function msToTime(ms) {
  let milliseconds = (ms).toFixed(1);
  let seconds = (ms / 1000).toFixed(1);
  let minutes = (ms / (1000 * 60)).toFixed(1);
  let hours = (ms / (1000 * 60 * 60)).toFixed(1);
  let days = (ms / (1000 * 60 * 60 * 24)).toFixed(1);

  if (milliseconds < 1000) return milliseconds + "ms";
  else if (seconds < 60) return seconds + "s";
  else if (minutes < 60) return minutes + "m";
  else if (hours < 24) return hours + "hr";
  else return days + " Days"
}

export default function HistoryList(props){
  return (

            <ul
              role="list"
              className="divide-y divide-gray-100 overflow-hidden bg-white shadow-sm ring-1 ring-gray-900/5"
            >
              {props.history.map((h) => (
                <li key={h.id} className="relative flex justify-between gap-x-6 px-4 py-5 hover:bg-gray-50 sm:px-6">
                  <div className="flex min-w-0 gap-x-4">
                    <div className="flex flex-col justify-center">
                      <p className="flex text-xs text-gray-500 justify-center">{ DateTime.fromMillis(h.timestamp / 1000000).toLocaleString(DateTime.TIME_WITH_SECONDS)}</p>
                      <p className="flex text-xs mt-1 text-gray-400 justify-center">{ DateTime.fromMillis(h.timestamp / 1000000).toLocaleString(DateTime.DATE_SHORT)}</p>
                    </div>
                    <div className="min-w-0 flex-col justify-center">
                      <pre className="whitespace-pre-wrap"><code className="text-sm">{h.command}</code></pre>
                      <p className="mt-1 flex text-xs leading-5 text-gray-500">
                        <span className="relative truncate ">
                          {h.user}
                        </span>

                        <span>&nbsp;on&nbsp;</span>

                        <span className="relative truncate ">
                          {h.host}
                        </span>

                        <span>&nbsp;in&nbsp;</span>

                        <span className="relative truncate ">
                          {h.cwd}
                        </span>
                      </p>
                    </div>
                  </div>
                  <div className="flex shrink-0 items-center gap-x-4">
                    <div className="hidden sm:flex sm:flex-col sm:items-end">
                      <p className="text-sm leading-6 text-gray-900">{h.exit}</p>
                      {h.duration ? (
                        <p className="mt-1 text-xs leading-5 text-gray-500">
                          <time dateTime={h.duration}>{msToTime(h.duration / 1000000)}</time>
                        </p>
                      ) : (
                        <div className="mt-1 flex items-center gap-x-1.5">
                          <div className="flex-none rounded-full bg-emerald-500/20 p-1">
                            <div className="h-1.5 w-1.5 rounded-full bg-emerald-500" />
                          </div>
                          <p className="text-xs leading-5 text-gray-500">Online</p>
                        </div>
                      )}
                    </div>
                    <ChevronRightIcon className="h-5 w-5 flex-none text-gray-400" aria-hidden="true" />
                  </div>
                </li>
              ))}
            </ul>
  );
}
