import { useState, useEffect } from "react";

import CodeBlock from "@/components/CodeBlock";
import HistoryRow from "@/components/history/HistoryRow";
import { inspectCommandHistory } from "@/state/models";

export default function HistoryInspect({ history }: any) {
  let [other, setOther] = useState([]);

  useEffect(() => {
    (async () => {
      let inspect = await inspectCommandHistory(history);
      setOther(inspect.other);
    })();
  }, []);

  return (
    <div className="overflow-y-auto">
      <CodeBlock code={history.command} language="bash" />

      <div>
        {other &&
          other.map((i: any) => {
            return <HistoryRow h={i} />;
          })}
      </div>
    </div>
  );
}
