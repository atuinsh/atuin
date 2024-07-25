import { invoke } from "@tauri-apps/api/core";
import Database from "@tauri-apps/plugin-sql";

export class User {
  username: string | null;

  constructor(username: string) {
    this.username = username;
  }

  isLoggedIn(): boolean {
    return this.username !== "" && this.username !== null;
  }
}

export const DefaultUser: User = new User("");

export interface HomeInfo {
  historyCount: number;
  recordCount: number;
  lastSyncTime: Date | null;
  recentCommands: ShellHistory[];
  topCommands: ShellHistory[];
}

export const DefaultHomeInfo: HomeInfo = {
  historyCount: 0,
  recordCount: 0,
  lastSyncTime: new Date(),
  recentCommands: [],
  topCommands: [],
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

// Not yet complete. Not all types are defined here.
// Gonna hold off until the settings refactoring.
export interface Settings {
  auto_sync: boolean;
  update_check: boolean;
  sync_address: string;
  sync_frequency: string;
  db_path: string;
  record_store_path: string;
  key_path: string;
  session_path: string;
  shell_up_key_binding: boolean;
  inline_height: number;
  invert: boolean;
  show_preview: boolean;
  max_preview_height: number;
  show_help: boolean;
  show_tabs: boolean;
  word_chars: string;
  scroll_context_lines: number;
  history_format: string;
  prefers_reduced_motion: boolean;
  store_failed: boolean;
  secrets_filter: boolean;
  workspaces: boolean;
  ctrl_n_shortcuts: boolean;
  network_connect_timeout: number;
  network_timeout: number;
  local_timeout: number;
  enter_accept: boolean;
  smart_sort: boolean;
  sync: Sync;
}

interface Sync {
  records: boolean;
}

// Define other interfaces (Dialect, Timezone, Style, SearchMode, FilterMode, ExitMode, KeymapMode, CursorStyle, WordJumpMode, RegexSet, Stats) accordingly.

export async function inspectCommandHistory(
  h: ShellHistory,
): Promise<InspectHistory> {
  const settings: Settings = await invoke("cli_settings");
  const db = await Database.load("sqlite:" + settings.db_path);

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
  const settings: Settings = await invoke("cli_settings");
  const db = await Database.load("sqlite:" + settings.db_path);

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
