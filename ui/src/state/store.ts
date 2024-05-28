import { create } from "zustand";
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
  Settings,
} from "./models";

import { invoke } from "@tauri-apps/api/core";
import { sessionToken, settings } from "./client";

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
  refreshUser: () => void;
  refreshShellHistory: (query?: string) => void;
  historyNextPage: (query?: string) => void;
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

  refreshUser: async () => {
    let config = await settings();
    let session = await sessionToken();
    let url = config.sync_address + "/api/v0/me";

    let res = await fetch(url, {
      headers: {
        Authorization: `Token ${session}`,
      },
    });
    let me = await res.json();

    set({ user: me });
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
}));
