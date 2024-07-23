import React, { useEffect } from "react";
import { formatRelative } from "date-fns";
import { Tooltip as ReactTooltip } from "react-tooltip";

import { AtuinState, useStore } from "@/state/store";
import { useToast } from "@/components/ui/use-toast";
import { ToastAction } from "@/components/ui/toast";
import { invoke } from "@tauri-apps/api/core";
import {
  Card,
  CardHeader,
  CardBody,
  Listbox,
  ListboxItem,
} from "@nextui-org/react";

import {
  Bar,
  BarChart,
  CartesianGrid,
  LabelList,
  XAxis,
  YAxis,
} from "recharts";
import { ChartConfig, ChartContainer } from "@/components/ui/chart";

import { Clock, Terminal } from "lucide-react";

import ActivityCalendar from "react-activity-calendar";
import HistoryRow from "@/components/history/HistoryRow";
import { ShellHistory } from "@/state/models";

function StatCard({ name, stat }: any) {
  return (
    <Card shadow="sm">
      <CardHeader>
        <h3 className="uppercase text-gray-500">{name}</h3>
      </CardHeader>
      <CardBody>
        <h2 className="font-bold text-xl">{stat}</h2>
      </CardBody>
    </Card>
  );
}

function TopChart({ chartData }: any) {
  const chartConfig = {
    command: {
      label: "Command",
      color: "#c4edde",
    },
  } satisfies ChartConfig;

  return (
    <ChartContainer config={chartConfig} className="max-h-72">
      <BarChart
        accessibilityLayer
        data={chartData}
        layout="vertical"
        margin={{
          right: 16,
        }}
      >
        <CartesianGrid horizontal={false} />
        <YAxis
          dataKey="command"
          type="category"
          tickLine={false}
          tickMargin={10}
          axisLine={false}
          tickFormatter={(value) => value.slice(0, 3)}
          hide
        />
        <XAxis dataKey="count" type="number" hide />
        <Bar dataKey="count" layout="vertical" fill="#c4edde" radius={4}>
          <LabelList
            dataKey="command"
            position="insideLeft"
            offset={8}
            className="fill-[--color-label]"
            fontSize={12}
          />
          <LabelList
            dataKey="count"
            position="right"
            offset={8}
            className="fill-foreground"
            fontSize={12}
          />
        </Bar>
      </BarChart>
    </ChartContainer>
  );
}

function Header({ name }: any) {
  let greeting = name && name.length > 0 ? "Hey, " + name + "!" : "Hey!";

  return (
    <div className="md:flex md:items-center md:justify-between">
      <div className="flex-1">
        <h2 className="text-2xl font-bold leading-7 text-gray-900 sm:truncate sm:text-3xl sm:tracking-tight">
          {greeting}
        </h2>
        <h3 className="text-xl leading-7 text-gray-900 pt-4">
          Welcome to{" "}
          <a
            href="https://atuin.sh"
            target="_blank"
            rel="noopener noreferrer nofollow"
          >
            Atuin
          </a>
          .
        </h3>
      </div>
    </div>
  );
}

const explicitTheme = {
  light: ["#f0f0f0", "#c4edde", "#7ac7c4", "#f73859", "#384259"],
  dark: ["#f0f0f0", "#c4edde", "#7ac7c4", "#f73859", "#384259"],
};

export default function Home() {
  const homeInfo = useStore((state: AtuinState) => state.homeInfo);
  const user = useStore((state: AtuinState) => state.user);
  const calendar = useStore((state: AtuinState) => state.calendar);
  const runbooks = useStore((state: AtuinState) => state.runbooks);
  const weekStart = useStore((state: AtuinState) => state.weekStart);

  const refreshHomeInfo = useStore(
    (state: AtuinState) => state.refreshHomeInfo,
  );
  const refreshUser = useStore((state: AtuinState) => state.refreshUser);
  const refreshCalendar = useStore(
    (state: AtuinState) => state.refreshCalendar,
  );
  const refreshRunbooks = useStore(
    (state: AtuinState) => state.refreshRunbooks,
  );

  const { toast } = useToast();

  useEffect(() => {
    refreshHomeInfo();
    refreshUser();
    refreshCalendar();
    refreshRunbooks();

    console.log(homeInfo);

    let setup = async () => {
      let installed = await invoke("is_cli_installed");
      console.log("CLI installation status:", installed);

      if (!installed) {
        toast({
          title: "Atuin CLI",
          description: "CLI not detected - install?",
          action: (
            <ToastAction
              altText="Install"
              onClick={() => {
                let install = async () => {
                  toast({
                    title: "Atuin CLI",
                    description: "Install in progress...",
                  });

                  console.log("Installing CLI...");
                  await invoke("install_cli");

                  console.log("Setting up plugin...");
                  await invoke("setup_cli");

                  toast({
                    title: "Atuin CLI",
                    description: "Installation complete",
                  });
                };
                install();
              }}
            >
              Install
            </ToastAction>
          ),
        });
      }
    };

    setup();
  }, []);

  if (!homeInfo) {
    return <div>Loading...</div>;
  }

  return (
    <div className="w-full flex-1 flex-col p-4 overflow-y-auto">
      <div className="pl-10">
        <Header name={user.username} />
      </div>
      <div className="p-10 grid grid-cols-4 gap-4">
        <StatCard
          name="Last Sync"
          stat={
            (homeInfo.lastSyncTime &&
              formatRelative(homeInfo.lastSyncTime, new Date())) ||
            "Never"
          }
        />
        <StatCard
          name="Total Commands"
          stat={homeInfo.historyCount.toLocaleString()}
        />
        <StatCard
          name="Total Runbooks"
          stat={runbooks.length.toLocaleString()}
        />
        <StatCard
          name="Other Records"
          stat={homeInfo.recordCount - homeInfo.historyCount}
        />

        <Card shadow="sm" className="col-span-3">
          <CardHeader>
            <h2 className="uppercase text-gray-500">Activity graph</h2>
          </CardHeader>
          <CardBody>
            <ActivityCalendar
              hideTotalCount
              theme={explicitTheme}
              data={calendar}
              weekStart={weekStart as any}
              renderBlock={(block, activity) =>
                React.cloneElement(block, {
                  "data-tooltip-id": "react-tooltip",
                  "data-tooltip-html": `${activity.count} commands on ${activity.date}`,
                })
              }
            />
            <ReactTooltip id="react-tooltip" />
          </CardBody>
        </Card>

        <Card shadow="sm">
          <CardHeader>
            <h2 className="uppercase text-gray-500">Quick actions </h2>
          </CardHeader>

          <CardBody>
            <Listbox variant="flat" aria-label="Quick actions">
              <ListboxItem
                key="new-runbook"
                description="Create an executable runbook"
                startContent={<Terminal />}
              >
                New runbook
              </ListboxItem>
              <ListboxItem
                key="shell-history"
                description="Search and explore shell history"
                startContent={<Clock />}
              >
                Shell History
              </ListboxItem>
            </Listbox>
          </CardBody>
        </Card>

        <Card shadow="sm" className="col-span-2">
          <CardHeader>
            <h2 className="uppercase text-gray-500">Recent commands</h2>
          </CardHeader>
          <CardBody>
            {homeInfo.recentCommands?.map((i: ShellHistory) => {
              return <HistoryRow compact h={i} />;
            })}
          </CardBody>
        </Card>

        <Card shadow="sm" className="col-span-2">
          <CardHeader>
            <h2 className="uppercase text-gray-500">Top commands</h2>
          </CardHeader>
          <CardBody>
            <TopChart chartData={homeInfo.topCommands} />
          </CardBody>
        </Card>
      </div>
    </div>
  );
}
