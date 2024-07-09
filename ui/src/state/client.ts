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

export async function login(
  username: string,
  password: string,
  key: string,
): Promise<string> {
  return await invoke("login", { username, password, key });
}

export async function logout(): Promise<string> {
  return await invoke("logout");
}

export async function register(
  username: string,
  email: string,
  password: string,
): Promise<string> {
  return await invoke("register", { username, email, password });
}
