// At some point, I'd like to replace some of the Atuin calls
// with separate state handling here

import { invoke } from "@tauri-apps/api/core";
import { Settings } from "@/state/models";

export async function sessionToken(): Promise<String> {
  return await invoke("session");
}

export async function settings(): Promise<Settings> {
  return await invoke("config");
}
