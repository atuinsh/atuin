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

export interface ShellHistory {
  id: string;
  timestamp: number;
  command: string;
  user: string;
  host: string;
  cwd: string;
  duration: number;
}

export interface Alias {
  name: string;
  value: string;
}
