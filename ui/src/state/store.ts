import { create } from "zustand";
import { persist } from "zustand/middleware";

import { parseISO } from "date-fns";

import { fetch } from "@tauri-apps/plugin-http";

import {
  User,
  DefaultUser,
  HomeInfo,
  DefaultHomeInfo,
  Alias,
  ShellHistory,
  Var,
} from "./models";

import { invoke } from "@tauri-apps/api/core";
import { sessionToken, settings } from "./client";
import { getWeekInfo } from "@/lib/utils";
import Runbook from "./runbooks/runbook";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";

export class TerminalData {
  terminal: Terminal;
  fitAddon: FitAddon;

  constructor(terminal: Terminal, fit: FitAddon) {
    this.terminal = terminal;
    this.fitAddon = fit;
  }
}

// I'll probs want to slice this up at some point, but for now a
// big blobby lump of state is fine.
// Totally just hoping that structure will be emergent in the future.
export interface AtuinState {
  user: User;
  homeInfo: HomeInfo;
  aliases: Alias[];
  vars: Var[];
  shellHistory: ShellHistory[];
  calendar: any[];
  weekStart: number;
  runbooks: Runbook[];
  currentRunbook: string | null;

  refreshHomeInfo: () => void;
  refreshCalendar: () => void;
  refreshAliases: () => void;
  refreshVars: () => void;
  refreshUser: () => void;
  refreshRunbooks: () => void;
  refreshShellHistory: (query?: string) => void;
  historyNextPage: (query?: string) => void;

  setCurrentRunbook: (id: String) => void;
  setPtyTerm: (pty: string, terminal: any) => void;
  newPtyTerm: (pty: string) => TerminalData;
  cleanupPtyTerm: (pty: string) => void;

  terminals: { [pty: string]: TerminalData };

  // Store ephemeral state for runbooks, that is not persisted to the database
  runbookInfo: { [runbook: string]: { ptys: number } };
  incRunbookPty: (runbook: string) => void;
  decRunbookPty: (runbook: string) => void;
}

let state = (set: any, get: any): AtuinState => ({
  user: DefaultUser,
  homeInfo: DefaultHomeInfo,
  aliases: [],
  vars: [],
  shellHistory: [],
  calendar: [],
  runbooks: [],
  currentRunbook: "",
  terminals: {},
  runbookInfo: {},

  weekStart: getWeekInfo().firstDay,

  refreshAliases: () => {
    invoke("aliases").then((aliases: any) => {
      set({ aliases: aliases });
    });
  },

  refreshCalendar: () => {
    invoke("history_calendar").then((calendar: any) => {
      set({ calendar: calendar });
    });
  },

  refreshVars: () => {
    invoke("vars").then((vars: any) => {
      set({ vars: vars });
    });
  },

  refreshRunbooks: async () => {
    let runbooks = await Runbook.all();
    set({ runbooks });
  },

  refreshShellHistory: (query?: string) => {
    if (query) {
      invoke("search", { query: query })
        .then((res: any) => {
          set({ shellHistory: res });
        })
        .catch((e) => {
          console.log(e);
        });
    } else {
      invoke("list").then((res: any) => {
        set({ shellHistory: res });
      });
    }
  },

  refreshHomeInfo: () => {
    invoke("home_info")
      .then((res: any) => {
        console.log(res);
        set({
          homeInfo: {
            historyCount: res.history_count,
            recordCount: res.record_count,
            lastSyncTime: (res.last_sync && parseISO(res.last_sync)) || null,
            recentCommands: res.recent_commands,
            topCommands: res.top_commands.map((top: any) => ({
              command: top[0],
              count: top[1],
            })),
          },
        });
      })
      .catch((e) => {
        console.log(e);
      });
  },

  refreshUser: async () => {
    let config = await settings();
    let session;

    try {
      session = await sessionToken();
    } catch (e) {
      console.log("Not logged in, so not refreshing user");
      set({ user: DefaultUser });
      return;
    }
    let url = config.sync_address + "/api/v0/me";

    let res = await fetch(url, {
      headers: {
        Authorization: `Token ${session}`,
      },
    });
    let me = await res.json();

    set({ user: new User(me.username) });
  },

  historyNextPage: (query?: string) => {
    let history = get().shellHistory;
    let offset = history.length - 1;

    if (query) {
      invoke("search", { query: query, offset: offset })
        .then((res: any) => {
          set({ shellHistory: [...history, ...res] });
        })
        .catch((e) => {
          console.log(e);
        });
    } else {
      invoke("list", { offset: offset }).then((res: any) => {
        set({ shellHistory: [...history, ...res] });
      });
    }
  },

  setCurrentRunbook: (id: String) => {
    set({ currentRunbook: id });
  },

  setPtyTerm: (pty: string, terminal: TerminalData) => {
    set({
      terminals: { ...get().terminals, [pty]: terminal },
    });
  },

  cleanupPtyTerm: (pty: string) => {
    set((state: AtuinState) => {
      const terminals = Object.keys(state.terminals).reduce(
        (terms: { [pty: string]: TerminalData }, key) => {
          if (key !== pty) {
            terms[key] = state.terminals[key];
          }
          return terms;
        },
        {},
      );

      return { terminals };
    });
  },

  newPtyTerm: (pty: string) => {
    let terminal = new Terminal();

    // TODO: fallback to canvas, also some sort of setting to allow disabling webgl usage
    // probs fine for now though, it's widely supported. maybe issues on linux.
    terminal.loadAddon(new WebglAddon());

    let fitAddon = new FitAddon();
    terminal.loadAddon(fitAddon);

    const onResize = (size: { cols: number; rows: number }) => {
      invoke("pty_resize", {
        pid: pty,
        cols: size.cols,
        rows: size.rows,
      });
    };

    terminal.onResize(onResize);

    let td = new TerminalData(terminal, fitAddon);

    set({
      terminals: { ...get().terminals, [pty]: td },
    });

    return td;
  },

  incRunbookPty: (runbook: string) => {
    set((state: AtuinState) => {
      let oldVal = state.runbookInfo[runbook] || { ptys: 0 };
      let newVal = { ptys: oldVal.ptys + 1 };
      console.log(newVal);

      return {
        runbookInfo: {
          ...state.runbookInfo,
          [runbook]: newVal,
        },
      };
    });
  },

  decRunbookPty: (runbook: string) => {
    set((state: AtuinState) => {
      let newVal = state.runbookInfo[runbook];
      if (!newVal) {
        return;
      }

      newVal.ptys--;

      return {
        runbookInfo: {
          ...state.runbookInfo,
          [runbook]: newVal,
        },
      };
    });
  },
});

export const useStore = create<AtuinState>()(
  persist(state, {
    name: "atuin-storage",

    // don't serialize the terminals map
    // it won't work as JSON. too cyclical
    partialize: (state) =>
      Object.fromEntries(
        Object.entries(state).filter(([key]) => !["terminals"].includes(key)),
      ),
  }),
);
