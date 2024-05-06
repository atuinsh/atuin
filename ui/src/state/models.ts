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

  constructor(
    id: string,
    timestamp: number,
    command: string,
    user: string,
    host: string,
    cwd: string,
    duration: number,
  ) {
    this.id = id;
    this.timestamp = timestamp;
    this.command = command;
    this.user = user;
    this.host = host;
    this.cwd = cwd;
    this.duration = duration;
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

export async function inspectHistory(id: string): Promise<any> {
  const db = await Database.load(
    "sqlite:/Users/ellie/.local/share/atuin/history.db",
  );

  let res = await db.select("select * from history where id=$1", [id]);

  return res;
}
