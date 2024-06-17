import "./App.css";

import { useState, ReactElement } from "react";
import { useStore } from "@/state/store";

import Button, { ButtonStyle } from "@/components/Button";
import { Toaster } from "@/components/ui/toaster";

import { Dialog, DialogContent, DialogTrigger } from "@/components/ui/dialog";

import {
  HomeIcon,
  ClockIcon,
  WrenchScrewdriverIcon,
} from "@heroicons/react/24/outline";
import Logo from "./assets/logo-light.svg";

function classNames(...classes: any) {
  return classes.filter(Boolean).join(" ");
}

import Home from "./pages/Home.tsx";
import History from "./pages/History.tsx";
import Dotfiles from "./pages/Dotfiles.tsx";
import LoginOrRegister from "./components/LoginOrRegister.tsx";

enum Section {
  Home,
  History,
  Dotfiles,
}

function renderMain(section: Section): ReactElement {
  switch (section) {
    case Section.Home:
      return <Home />;
    case Section.History:
      return <History />;
    case Section.Dotfiles:
      return <Dotfiles />;
  }
}

function App() {
  // routers don't really work in Tauri. It's not a browser!
  // I think hashrouter may work, but I'd rather avoiding thinking of them as
  // pages
  const [section, setSection] = useState(Section.Home);
  const user = useStore((state) => state.user);
  console.log(user);

  const navigation = [
    {
      name: "Home",
      icon: HomeIcon,
      section: Section.Home,
    },
    {
      name: "History",
      icon: ClockIcon,
      section: Section.History,
    },
    {
      name: "Dotfiles",
      icon: WrenchScrewdriverIcon,
      section: Section.Dotfiles,
    },
  ];

  return (
    <div>
      <div className="fixed inset-y-0 z-50 flex w-60 flex-col">
        <div className="flex grow flex-col gap-y-5 overflow-y-auto border-r border-gray-200 bg-white px-6 pb-4">
          <div className="flex h-16 shrink-0 items-center">
            <img className="h-8 w-auto" src={Logo} alt="Atuin" />
          </div>
          <nav className="flex flex-1 flex-col">
            <ul role="list" className="flex flex-1 flex-col gap-y-7">
              <li>
                <ul role="list" className="-mx-2 space-y-1 w-full">
                  {navigation.map((item) => (
                    <li key={item.name}>
                      <button
                        onClick={() => setSection(item.section)}
                        className={classNames(
                          section == item.section
                            ? "bg-gray-50 text-green-600"
                            : "text-gray-700 hover:text-green-600 hover:bg-gray-50",
                          "group flex gap-x-3 rounded-md p-2 text-sm leading-6 font-semibold w-full",
                        )}
                      >
                        <item.icon
                          className={classNames(
                            section == item.section
                              ? "text-green-600"
                              : "text-gray-400 group-hover:text-green-600",
                            "h-6 w-6 shrink-0",
                          )}
                          aria-hidden="true"
                        />
                        {item.name}
                      </button>
                    </li>
                  ))}
                </ul>
              </li>
              <li className="mt-auto">
                {user && !user.isLoggedIn() && (
                  <Dialog>
                    <DialogTrigger className="w-full">
                      <Button
                        text={"Login or Register"}
                        style={ButtonStyle.PrimarySmFill}
                      />
                    </DialogTrigger>
                    <DialogContent>
                      <LoginOrRegister />
                    </DialogContent>
                  </Dialog>
                )}
              </li>
            </ul>
          </nav>
        </div>
      </div>

      {renderMain(section)}
      <Toaster />
    </div>
  );
}

export default App;
