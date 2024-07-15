import Editor from "@/components/runbooks/editor/Editor";
import List from "@/components/runbooks/List";

export default function Runbooks() {
  return (
    <div className="w-full flex flex flex-row ">
      <List />
      <Editor />
    </div>
  );
}
