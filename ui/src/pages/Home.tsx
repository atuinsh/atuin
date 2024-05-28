import { useEffect } from "react";
import { formatRelative } from "date-fns";

import { useStore } from "@/state/store";

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

export default function Home() {
  const homeInfo = useStore((state) => state.homeInfo);
  const user = useStore((state) => state.user);
  const refreshHomeInfo = useStore((state) => state.refreshHomeInfo);
  const refreshUser = useStore((state) => state.refreshUser);

  useEffect(() => {
    refreshHomeInfo();
    refreshUser();
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
                stat: formatRelative(homeInfo.lastSyncTime, new Date()),
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
      </div>
    </div>
  );
}
