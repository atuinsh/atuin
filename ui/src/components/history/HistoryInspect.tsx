import { useState, useEffect } from "react";

import PacmanLoader from "react-spinners/PacmanLoader";

import CodeBlock from "@/components/CodeBlock";
import HistoryRow from "@/components/history/HistoryRow";
import { ShellHistory, inspectCommandHistory } from "@/state/models";

function renderLoading() {
  return (
    <div className="flex items-center justify-center h-full">
      <PacmanLoader color="#26bd65" />
    </div>
  );
}

export default function HistoryInspect({ history }: any) {
  let [other, setOther] = useState<ShellHistory[]>([]);

  useEffect(() => {
    (async () => {
      let inspect = await inspectCommandHistory(history);
      setOther(inspect.other);
    })();
  }, []);

  if (other.length == 0) return renderLoading();

  return (
    <div className="overflow-y-auto">
      <CodeBlock code={history.command} language="bash" />

      <div>
        {other.map((i: any) => {
          return <HistoryRow h={i} />;
        })}
      </div>
    </div>
  );
}
