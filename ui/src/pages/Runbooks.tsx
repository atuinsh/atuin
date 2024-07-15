import Editor from "@/components/runbooks/editor/Editor";
import List from "@/components/runbooks/List";
import { useStore } from "@/state/store";

export default function Runbooks() {
  const currentRunbook = useStore((store) => store.currentRunbook);

  return (
    <div className="w-full flex flex-row ">
      <List />
      {currentRunbook && <Editor />}

      {!currentRunbook && (
        <div className="flex align-middle justify-center flex-col h-screen w-full">
          <h1 className="text-center">Select or create a runbook</h1>
        </div>
      )}
    </div>
  );
}
