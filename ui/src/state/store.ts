import { create } from "zustand";
import { parseISO } from "date-fns";

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

// I'll probs want to slice this up at some point, but for now a
// big blobby lump of state is fine.
// Totally just hoping that structure will be emergent in the future.
interface AtuinState {
  user: User;
  homeInfo: HomeInfo;
  aliases: Alias[];
  vars: Var[];
  shellHistory: ShellHistory[];

  refreshHomeInfo: () => void;
  refreshAliases: () => void;
  refreshVars: () => void;
  refreshShellHistory: (query?: string) => void;
  historyNextPage: () => void;
}

export const useStore = create<AtuinState>()((set, get) => ({
  user: DefaultUser,
  homeInfo: DefaultHomeInfo,
  aliases: [],
  vars: [],
  shellHistory: [],

  refreshAliases: () => {
    invoke("aliases").then((aliases: any) => {
      set({ aliases: aliases });
    });
  },

  refreshVars: () => {
    invoke("vars").then((vars: any) => {
      set({ vars: vars });
    });
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
        set({
          homeInfo: {
            historyCount: res.history_count,
            recordCount: res.record_count,
            lastSyncTime: parseISO(res.last_sync),
          },
        });
      })
      .catch((e) => {
        console.log(e);
      });
  },

  historyNextPage: (query?: string) => {
    let history = get().shellHistory;
    let minTimestamp = history[history.length - 1].timestamp;
    console.log(minTimestamp);

    if (query) {
      invoke("search", { query: query, minTimestamp: minTimestamp })
        .then((res: any) => {
          set({ shellHistory: res });
        })
        .catch((e) => {
          console.log(e);
        });
    } else {
      invoke("list", { minTimestamp: minTimestamp }).then((res: any) => {
        console.log(res, history);
        set({ shellHistory: [...history, ...res] });
      });
    }
  },
}));
