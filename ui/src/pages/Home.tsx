import React, { useEffect, useState } from "react";
import { formatRelative } from "date-fns";
import { Tooltip as ReactTooltip } from "react-tooltip";

import { useStore } from "@/state/store";
import { useToast } from "@/components/ui/use-toast";
import { invoke } from "@tauri-apps/api/core";

import ActivityCalendar from "react-activity-calendar";

function Stats({ stats }: any) {
  return (
    <div>
      <dl className="mt-5 grid grid-cols-1 gap-5 sm:grid-cols-3">
        {stats.map((item: any) => (
          <div
            key={item.name}
            className="overflow-hidden rounded-lg bg-white px-4 py-5 shadow sm:p-6"
          >
            <dt className="truncate text-sm font-medium text-gray-500">
              {item.name}
            </dt>
            <dd className="mt-1 text-xl font-semibold tracking-tight text-gray-900">
              {item.stat}
            </dd>
          </div>
        ))}
      </dl>
    </div>
  );
}

function Header({ name }: any) {
  let greeting = name && name.length > 0 ? "Hey, " + name + "!" : "Hey!";

  return (
    <div className="md:flex md:items-center md:justify-between">
      <div className="min-w-0 flex-1">
        <h2 className="text-2xl font-bold leading-7 text-gray-900 sm:truncate sm:text-3xl sm:tracking-tight">
          {greeting}
        </h2>
        <h3 className="text-xl leading-7 text-gray-900 pt-4">
          Welcome to Atuin.
        </h3>
      </div>
    </div>
  );
}

const explicitTheme: ThemeInput = {
  light: ["#f0f0f0", "#c4edde", "#7ac7c4", "#f73859", "#384259"],
  dark: ["#383838", "#4D455D", "#7DB9B6", "#F5E9CF", "#E96479"],
};

export default function Home() {
  const [weekStart, setWeekStart] = useState(0);

  const homeInfo = useStore((state) => state.homeInfo);
  const user = useStore((state) => state.user);
  const calendar = useStore((state) => state.calendar);

  const refreshHomeInfo = useStore((state) => state.refreshHomeInfo);
  const refreshUser = useStore((state) => state.refreshUser);
  const refreshCalendar = useStore((state) => state.refreshCalendar);

  const { toast } = useToast();

  useEffect(() => {
    let locale = new Intl.Locale(navigator.language);
    let weekinfo = locale.getWeekInfo();
    setWeekStart(weekinfo.firstDay);

    refreshHomeInfo();
    refreshUser();
    refreshCalendar();

    let setup = async () => {
      let installed = await invoke("is_cli_installed");
      console.log("CLI installation status:", installed);

      if (!installed) {
        toast({
          title: "Atuin CLI",
          description: "Started CLI setup and installation...",
        });

        console.log("Installing CLI...");
        await invoke("install_cli");

        console.log("Setting up plugin...");
        await invoke("setup_cli");

        toast({
          title: "Atuin CLI",
          description: "Installation complete",
        });
      }
    };

    setup();
  }, []);

  if (!homeInfo) {
    return <div>Loading...</div>;
  }

  return (
    <div className="pl-60">
      <div className="p-10">
        <Header name={user.username} />

        <div className="pt-10">
          <h2 className="text-xl font-bold">Sync</h2>
          <Stats
            stats={[
              {
                name: "Last Sync",
                stat:
                  (homeInfo.lastSyncTime &&
                    formatRelative(homeInfo.lastSyncTime, new Date())) ||
                  "Never",
              },
              {
                name: "Total history records",
                stat: homeInfo.historyCount.toLocaleString(),
              },
              {
                name: "Other records",
                stat: homeInfo.recordCount - homeInfo.historyCount,
              },
            ]}
          />
        </div>

        <div className="pt-10">
          <ActivityCalendar
            theme={explicitTheme}
            data={calendar}
            weekStart={weekStart}
            renderBlock={(block, activity) =>
              React.cloneElement(block, {
                "data-tooltip-id": "react-tooltip",
                "data-tooltip-html": `${activity.count} commands on ${activity.date}`,
              })
            }
            labels={{
              totalCount: "{{count}} history records in the last year",
            }}
          />
          <ReactTooltip id="react-tooltip" />
        </div>
      </div>
    </div>
  );
}
