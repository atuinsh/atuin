import CodeBlock from "@/components/CodeBlock";

export default function HistoryInspect({ history }: any) {
  return (
    <div>
      <CodeBlock code={history.command} language="bash" />
    </div>
  );
}
