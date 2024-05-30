export enum ButtonStyle {
  PrimarySm = "bg-emerald-500 hover:bg-emerald-600",
  PrimarySmFill = "bg-emerald-500 hover:bg-emerald-600 w-full text-sm",
}

interface ButtonProps {
  text: string;
  style: ButtonStyle;
}

export default function Button(props: ButtonProps) {
  return (
    <button
      type="button"
      className={`rounded ${props.style} px-2 py-1 font-semibold text-white shadow-sm focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-indigo-500`}
    >
      {props.text}
    </button>
  );
}
