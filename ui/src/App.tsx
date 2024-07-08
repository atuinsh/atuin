import "./App.css";

import { useState, ReactElement } from "react";
import { useStore } from "@/state/store";

import { Toaster } from "@/components/ui/toaster";
import { Icon } from "@iconify/react";

import { Dialog, DialogContent, DialogTrigger } from "@/components/ui/dialog";

import {
  HomeIcon,
  ClockIcon,
  WrenchScrewdriverIcon,
} from "@heroicons/react/24/outline";

import { ChevronRightSquare } from "lucide-react";

import Logo from "./assets/logo-light.svg";

function classNames(...classes: any) {
  return classes.filter(Boolean).join(" ");
}

import Home from "./pages/Home.tsx";
import History from "./pages/History.tsx";
import Dotfiles from "./pages/Dotfiles.tsx";
import LoginOrRegister from "./components/LoginOrRegister.tsx";
import Runbooks from "./pages/Runbooks.tsx";

import {
  Avatar,
  Button,
  ScrollShadow,
  Spacer,
  Tooltip,
} from "@nextui-org/react";
import { cn } from "@/lib/utils";
import { sectionItems } from "@/components/Sidebar/sidebar-items";
import Sidebar, { SidebarItem } from "@/components/Sidebar";
import icon from "@/assets/icon.svg";
import iconText from "@/assets/logo-light.svg";

enum Section {
  Home,
  History,
  Dotfiles,
  Runbooks,
}

function renderMain(section: Section): ReactElement {
  switch (section) {
    case Section.Home:
      return <Home />;
    case Section.History:
      return <History />;
    case Section.Dotfiles:
      return <Dotfiles />;
    case Section.Runbooks:
      return <Runbooks />;
  }
}

function App() {
  // routers don't really work in Tauri. It's not a browser!
  // I think hashrouter may work, but I'd rather avoiding thinking of them as
  // pages
  const [section, setSection] = useState(Section.Home);
  const user = useStore((state) => state.user);
  const [isCollapsed, setIsCollapsed] = useState(false);
  const isCompact = true;

  const navigation: SidebarItem[] = [
    {
      key: "personal",
      title: "Personal",
      items: [
        {
          key: "home",
          icon: "solar:home-2-linear",
          title: "Home",
          onPress: () => setSection(Section.Home),
        },
        {
          key: "runbooks",
          icon: "solar:notebook-linear",
          title: "Runbooks",
          onPress: () => {
            console.log("runbooks");
            setSection(Section.Runbooks);
          },
        },
        {
          key: "history",
          icon: "solar:history-outline",
          title: "History",
          onPress: () => setSection(Section.History),
        },
        {
          key: "dotfiles",
          icon: "solar:file-smile-linear",
          title: "Dotfiles",
          onPress: () => setSection(Section.Dotfiles),
        },
        {
          key: "settings",
          icon: "solar:settings-linear",
          title: "Settings",
          onPress: () => setSection(Section.Settings),
        },
      ],
    },
  ];

  return (
    <div className="flex h-dvh w-full">
      <div
        className={cn(
          "relative flex h-full w-72 flex-col !border-r-small border-divider p-6 transition-width",
          {
            "w-16 items-center px-2 py-6": isCollapsed,
          },
        )}
      >
        <div
          className={cn(
            "flex items-center gap-3 px-3",

            {
              "justify-center gap-0": isCollapsed,
            },
          )}
        >
          {isCollapsed && (
            <div className="flex h-8 w-8">
              <img src={icon} alt="icon" className="h-8 w-8" />
            </div>
          )}

          {!isCollapsed && (
            <div className="flex w-1/2">
              <img src={iconText} alt="icon" className="w-full" />
            </div>
          )}
        </div>
        <Spacer y={8} />

        <div className="flex items-center gap-3 px-3">
          <Avatar isBordered className="flex-none" size="sm" />
          <div
            className={cn("flex max-w-full flex-col", { hidden: isCollapsed })}
          >
            <p className="truncate text-small font-medium text-default-600">
              {user.username}
            </p>
            <p className="truncate text-tiny text-default-400">
              {user.bio || ""}
            </p>
          </div>
        </div>

        <ScrollShadow className="-mr-6 h-full max-h-full py-6 pr-6">
          <Sidebar
            defaultSelectedKey="home"
            isCompact={isCollapsed}
            items={navigation}
          />
        </ScrollShadow>

        <Spacer y={2} />

        <div
          className={cn("mt-auto flex flex-col", {
            "items-center": isCollapsed,
          })}
        >
          <Tooltip
            content={(isCollapsed && "Expand menu") || "Collapse menu"}
            isDisabled={!isCollapsed}
            placement="right"
          >
            <Button
              onClick={() => setIsCollapsed(!isCollapsed)}
              fullWidth
              className={cn(
                "justify-start truncate text-default-500 data-[hover=true]:text-foreground",
                {
                  "justify-center": isCollapsed,
                },
              )}
              isIconOnly={isCollapsed}
              startContent={
                isCollapsed ? null : (
                  <Icon
                    className="flex-none text-default-500"
                    icon="solar:round-double-alt-arrow-left-line-duotone"
                    width={24}
                  />
                )
              }
              variant="light"
            >
              {isCollapsed ? (
                <Icon
                  className="text-default-500"
                  icon="solar:round-double-alt-arrow-right-line-duotone"
                  width={24}
                />
              ) : (
                "Collapse menu"
              )}
            </Button>
          </Tooltip>

          <Tooltip
            content="Help & Feedback"
            isDisabled={!isCollapsed}
            placement="right"
          >
            <Button
              fullWidth
              className={cn(
                "justify-start truncate text-default-500 data-[hover=true]:text-foreground",
                {
                  "justify-center": isCollapsed,
                },
              )}
              isIconOnly={isCollapsed}
              startContent={
                isCollapsed ? null : (
                  <Icon
                    className="flex-none text-default-500"
                    icon="solar:info-circle-line-duotone"
                    width={24}
                  />
                )
              }
              variant="light"
            >
              {isCollapsed ? (
                <Icon
                  className="text-default-500"
                  icon="solar:info-circle-line-duotone"
                  width={24}
                />
              ) : (
                "Help & Information"
              )}
            </Button>
          </Tooltip>

          <Tooltip
            content="Log Out"
            isDisabled={!isCollapsed}
            placement="right"
          >
            <Button
              className={cn(
                "justify-start text-default-500 data-[hover=true]:text-foreground",
                {
                  "justify-center": isCollapsed,
                },
              )}
              isIconOnly={isCollapsed}
              startContent={
                isCollapsed ? null : (
                  <Icon
                    className="flex-none rotate-180 text-default-500"
                    icon="solar:minus-circle-line-duotone"
                    width={24}
                  />
                )
              }
              variant="light"
            >
              {isCollapsed ? (
                <Icon
                  className="rotate-180 text-default-500"
                  icon="solar:minus-circle-line-duotone"
                  width={24}
                />
              ) : (
                "Log Out"
              )}
            </Button>
          </Tooltip>
        </div>
      </div>

      {renderMain(section)}

      <Toaster />
    </div>
  );
}

export default App;
