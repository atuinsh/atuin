import { create } from "zustand";
import { parseISO } from "date-fns";

import {
  User,
  DefaultUser,
  HomeInfo,
  DefaultHomeInfo,
  Alias,
  ShellHistory,
} from "./models";

import { invoke } from "@tauri-apps/api/core";

// I'll probs want to slice this up at some point, but for now a
// big blobby lump of state is fine.
// Totally just hoping that structure will be emergent in the future.
interface AtuinState {
  user: User;
  homeInfo: HomeInfo;
  aliases: Alias[];
  shellHistory: ShellHistory[];

  refreshHomeInfo: () => void;
  refreshAliases: () => void;
  refreshShellHistory: (query?: string) => void;
}

export const useStore = create<AtuinState>()((set) => ({
  user: DefaultUser,
  homeInfo: DefaultHomeInfo,
  aliases: [],
  shellHistory: [],

  refreshAliases: () => {
    invoke("aliases").then((aliases: any) => {
      set({ aliases: aliases });
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
}));
