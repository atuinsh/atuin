import Aliases from "@/components/dotfiles/Aliases";

function Header() {
  return (
    <div className="md:flex md:items-center md:justify-between">
      <div className="min-w-0 flex-1">
        <h2 className="text-2xl font-bold leading-7 text-gray-900 sm:truncate sm:text-3xl sm:tracking-tight">
          Dotfiles
        </h2>
      </div>
    </div>
  );
}

export default function Dotfiles() {
  return (
    <div className="pl-60">
      <div className="p-10">
        <Header />
        Manage your shell aliases, variables and paths
        <Aliases />
      </div>
    </div>
  );
}
