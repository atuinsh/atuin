import { useState } from "react";
import Aliases from "@/components/dotfiles/Aliases";
import Vars from "@/components/dotfiles/Vars";

enum Section {
  Aliases,
  Vars,
  Snippets,
}

function renderDotfiles(current: Section) {
  switch (current) {
    case Section.Aliases:
      return <Aliases />;
    case Section.Vars:
      return <Vars />;
    case Section.Snippets:
      return <div />;
  }
}

interface HeaderProps {
  current: Section;
  setCurrent: (section: Section) => void;
}

interface TabsProps {
  current: Section;
  setCurrent: (section: Section) => void;
}

function Header({ current, setCurrent }: HeaderProps) {
  return (
    <div className="md:flex md:items-center md:justify-between">
      <div className="min-w-0 flex-1">
        <h2 className="text-2xl font-bold leading-7 text-gray-900 sm:truncate sm:text-3xl sm:tracking-tight">
          Dotfiles
        </h2>
      </div>

      <Tabs current={current} setCurrent={setCurrent} />
    </div>
  );
}

function classNames(...classes: any[]) {
  return classes.filter(Boolean).join(" ");
}

function Tabs({ current, setCurrent }: TabsProps) {
  let tabs = [
    {
      name: "Aliases",
      isCurrent: () => current === Section.Aliases,
      section: Section.Aliases,
    },
    {
      name: "Vars",
      isCurrent: () => current === Section.Vars,
      section: Section.Vars,
    },
    {
      name: "Snippets",
      isCurrent: () => current === Section.Snippets,
      section: Section.Snippets,
    },
  ];

  return (
    <div>
      <div>
        <nav className="flex space-x-4" aria-label="Tabs">
          {tabs.map((tab) => (
            <button
              onClick={() => {
                setCurrent(tab.section);
              }}
              key={tab.name}
              className={classNames(
                tab.isCurrent()
                  ? "bg-gray-100 text-gray-700"
                  : "text-gray-500 hover:text-gray-700",
                "rounded-md px-3 py-2 text-sm font-medium",
              )}
              aria-current={tab.isCurrent() ? "page" : undefined}
            >
              {tab.name}
            </button>
          ))}
        </nav>
      </div>
    </div>
  );
}

export default function Dotfiles() {
  let [current, setCurrent] = useState(Section.Aliases);
  console.log(current);

  return (
    <div className="w-full flex-1 flex-col p-4">
      <div className="p-10">
        <Header current={current} setCurrent={setCurrent} />
        Manage your shell aliases, variables and paths
        {renderDotfiles(current)}
      </div>
    </div>
  );
}
