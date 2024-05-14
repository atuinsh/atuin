import Database from "@tauri-apps/plugin-sql";

export interface User {
  username: string;
}

export const DefaultUser: User = {
  username: "",
};

export interface HomeInfo {
  historyCount: number;
  recordCount: number;
  lastSyncTime: Date;
}

export const DefaultHomeInfo: HomeInfo = {
  historyCount: 0,
  recordCount: 0,
  lastSyncTime: new Date(),
};

export class ShellHistory {
  id: string;
  timestamp: number;
  command: string;
  user: string;
  host: string;
  cwd: string;
  duration: number;
  exit: number;

  /// Pass a row straight from the database to this
  constructor(
    id: string,
    timestamp: number,
    command: string,
    hostuser: string,
    cwd: string,
    duration: number,
    exit: number,
  ) {
    this.id = id;
    this.timestamp = timestamp;
    this.command = command;

    let [host, user] = hostuser.split(":");
    this.user = user;
    this.host = host;

    this.cwd = cwd;
    this.duration = duration;
    this.exit = exit;
  }
}

export interface Alias {
  name: string;
  value: string;
}

export interface Var {
  name: string;
  value: string;
  export: boolean;
}

export interface InspectHistory {
  other: ShellHistory[];
}

export async function inspectCommandHistory(
  h: ShellHistory,
): Promise<InspectHistory> {
  const db = await Database.load(
    "sqlite:/Users/ellie/.local/share/atuin/history.db",
  );

  let other: any[] = await db.select(
    "select * from history where command=?1 order by timestamp desc",
    [h.command],
  );
  console.log(other);

  return {
    other: other.map(
      (i) =>
        new ShellHistory(
          i.id,
          i.timestamp,
          i.command,
          i.hostname,
          i.cwd,
          i.duration,
          i.exit,
        ),
    ),
  };
}

export async function inspectDirectoryHistory(
  h: ShellHistory,
): Promise<InspectHistory> {
  const db = await Database.load(
    "sqlite:/Users/ellie/.local/share/atuin/history.db",
  );

  let other: any[] = await db.select(
    "select * from history where cwd=?1 order by timestamp desc",
    [h.cwd],
  );
  console.log(other);

  return {
    other: other.map(
      (i) =>
        new ShellHistory(
          i.id,
          i.timestamp,
          i.command,
          i.hostname,
          i.cwd,
          i.duration,
          i.exit,
        ),
    ),
  };
}
