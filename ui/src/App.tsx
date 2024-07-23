import "./App.css";
import { open } from "@tauri-apps/plugin-shell";

import { useState, ReactElement } from "react";
import { useStore } from "@/state/store";

import { Toaster } from "@/components/ui/toaster";
import { KeyRoundIcon } from "lucide-react";
import { Icon } from "@iconify/react";

import Home from "./pages/Home.tsx";
import History from "./pages/History.tsx";
import Dotfiles from "./pages/Dotfiles.tsx";
import LoginOrRegister from "./components/LoginOrRegister.tsx";
import Runbooks from "./pages/Runbooks.tsx";

import {
  Avatar,
  User,
  Button,
  ScrollShadow,
  Spacer,
  Dropdown,
  DropdownItem,
  DropdownMenu,
  DropdownSection,
  DropdownTrigger,
  Modal,
  ModalContent,
  useDisclosure,
} from "@nextui-org/react";
import Sidebar, { SidebarItem } from "@/components/Sidebar";
import icon from "@/assets/icon.svg";
import { logout } from "./state/client.ts";

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
  const user = useStore((state: any) => state.user);
  const refreshUser = useStore((state: any) => state.refreshUser);
  const { isOpen, onOpen, onOpenChange } = useDisclosure();

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
      ],
    },
  ];

  return (
    <div
      className="flex w-screen select-none"
      style={{ maxWidth: "100vw", height: "calc(100dvh - 2rem)" }}
    >
      <div className="flex w-full">
        <div className="relative flex flex-col !border-r-small border-divider transition-width pb-6 pt-4 items-center">
          <div className="flex items-center gap-0 px-3 justify-center">
            <div className="flex h-8 w-8">
              <img src={icon} alt="icon" className="h-8 w-8" />
            </div>
          </div>

          <ScrollShadow className="-mr-6 h-full max-h-full py-6 pr-6">
            <Sidebar
              defaultSelectedKey="home"
              isCompact={true}
              items={navigation}
              className="z-50"
            />
          </ScrollShadow>

          <Spacer y={2} />

          <div className="flex items-center gap-3 px-3">
            <Dropdown showArrow placement="right-start">
              <DropdownTrigger>
                <Button disableRipple isIconOnly radius="full" variant="light">
                  <Avatar
                    isBordered
                    className="flex-none"
                    size="sm"
                    name={user.username || ""}
                  />
                </Button>
              </DropdownTrigger>
              <DropdownMenu aria-label="Custom item styles">
                <DropdownItem
                  key="profile"
                  isReadOnly
                  className="h-14 opacity-100"
                  textValue="Signed in as"
                >
                  <User
                    avatarProps={{
                      size: "sm",
                      name: user.username || "Anonymous User",
                      showFallback: true,
                      imgProps: {
                        className: "transition-none",
                      },
                    }}
                    classNames={{
                      name: "text-default-600",
                      description: "text-default-500",
                    }}
                    description={
                      user.bio || (user.username && "No bio") || "Sign up now"
                    }
                    name={user.username || "Anonymous User"}
                  />
                </DropdownItem>

                <DropdownItem
                  key="settings"
                  description="Configure Atuin"
                  startContent={
                    <Icon icon="solar:settings-linear" width={24} />
                  }
                >
                  Settings
                </DropdownItem>

                <DropdownSection aria-label="Help & Feedback">
                  <DropdownItem
                    key="help_and_feedback"
                    description="Get in touch"
                    onPress={() => open("https://forum.atuin.sh")}
                    startContent={
                      <Icon width={24} icon="solar:question-circle-linear" />
                    }
                  >
                    Help & Feedback
                  </DropdownItem>

                  {(user.username && (
                    <DropdownItem
                      key="logout"
                      startContent={
                        <Icon width={24} icon="solar:logout-broken" />
                      }
                      onClick={() => {
                        logout();
                        refreshUser();
                      }}
                    >
                      Log Out
                    </DropdownItem>
                  )) || (
                    <DropdownItem
                      key="signup"
                      description="Sync, backup and share your data"
                      className="bg-emerald-100"
                      startContent={<KeyRoundIcon size="18px" />}
                      onPress={onOpen}
                    >
                      Log in or Register
                    </DropdownItem>
                  )}
                </DropdownSection>
              </DropdownMenu>
            </Dropdown>
          </div>
        </div>

        {renderMain(section)}

        <Toaster />
        <Modal
          isOpen={isOpen}
          onOpenChange={onOpenChange}
          placement="top-center"
        >
          <ModalContent className="p-8">
            {(onClose) => (
              <>
                <LoginOrRegister onClose={onClose} />
              </>
            )}
          </ModalContent>
        </Modal>
      </div>
    </div>
  );
}

export default App;
