import { create } from "zustand";
import { persist, createJSONStorage } from "zustand/middleware";

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

// I'll probs want to slice this up at some point, but for now a
// big blobby lump of state is fine.
// Totally just hoping that structure will be emergent in the future.
interface AtuinState {
  user: User;
  homeInfo: HomeInfo;
  aliases: Alias[];
  vars: Var[];
  shellHistory: ShellHistory[];
  calendar: any[];
  weekStart: number;

  refreshHomeInfo: () => void;
  refreshCalendar: () => void;
  refreshAliases: () => void;
  refreshVars: () => void;
  refreshUser: () => void;
  refreshShellHistory: (query?: string) => void;
  historyNextPage: (query?: string) => void;
}

let state = (set, get): AtuinState => ({
  user: DefaultUser,
  homeInfo: DefaultHomeInfo,
  aliases: [],
  vars: [],
  shellHistory: [],
  calendar: [],
  weekStart: new Intl.Locale(navigator.language).getWeekInfo().firstDay,

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
            lastSyncTime: (res.last_sync && parseISO(res.last_sync)) || null,
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
});

export const useStore = create<AtuinState>()(
  persist(state, { name: "atuin-storage" }),
);
