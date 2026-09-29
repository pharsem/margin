import { invoke } from "@tauri-apps/api/core";

export type Item = {
  id: number;
  title: string;
  created_at: number;
  due_at: number | null;
  notified_at: number | null;
  done_at: number | null;
};

export type Snapshot = {
  open: Item[];
  done_today: Item[];
  collapsed: boolean;
  errors: string[];
  hotkey: string;
  focus_hotkey: string;
};

export type Parsed = { title: string; due_at: number | null };

export const api = {
  snapshot: () => invoke<Snapshot>("get_snapshot"),
  parse: (text: string) => invoke<Parsed>("parse_capture", { text }),
  submit: (text: string) => invoke<void>("submit_capture", { text }),
  cancel: () => invoke<void>("cancel_capture"),
  openCapture: () => invoke<void>("open_capture"),
  edit: (id: number) => invoke<void>("edit_item", { id }),
  done: (id: number) => invoke<void>("done_item", { id }),
  reopen: (id: number) => invoke<void>("reopen_item", { id }),
  snooze: (id: number, minutes: number) => invoke<void>("snooze_item", { id, minutes }),
  remove: (id: number) => invoke<void>("delete_item", { id }),
  setCollapsed: (collapsed: boolean) => invoke<void>("set_collapsed", { collapsed }),
  releaseFocus: () => invoke<void>("release_focus"),
};
