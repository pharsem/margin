import { invoke } from "@tauri-apps/api/core";

export type Item = {
  id: number;
  title: string;
  created_at: number;
  due_at: number | null;
  notified_at: number | null;
  done_at: number | null;
  url: string | null;
  source_app: string | null;
};

export type Suggestion = {
  text: string;
  url: string | null;
  source: "page" | "window" | "clipboard";
};

export type SessionStatus = "running" | "needs_input" | "done";

export type Session = {
  id: string;
  cwd: string;
  folder: string;
  prompt: string | null;
  title: string | null;
  entrypoint: string | null;
  status: SessionStatus;
  started_at: number;
  updated_at: number;
  waiting_since: number | null;
  message: string | null;
};

export type Snapshot = {
  sessions: Session[];
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
  submit: (text: string, url: string | null) => invoke<void>("submit_capture", { text, url }),
  cancel: () => invoke<void>("cancel_capture"),
  openCapture: () => invoke<void>("open_capture"),
  edit: (id: number) => invoke<void>("edit_item", { id }),
  done: (id: number) => invoke<void>("done_item", { id }),
  reopen: (id: number) => invoke<void>("reopen_item", { id }),
  snooze: (id: number, minutes: number) => invoke<void>("snooze_item", { id, minutes }),
  remove: (id: number) => invoke<void>("delete_item", { id }),
  setCollapsed: (collapsed: boolean) => invoke<void>("set_collapsed", { collapsed }),
  releaseFocus: () => invoke<void>("release_focus"),
  review: (id: string) => invoke<void>("review_session", { id }),
  focusSession: (id: string) => invoke<void>("focus_session", { id }),
  openUrl: (url: string) => invoke<void>("open_url", { url }),
};
