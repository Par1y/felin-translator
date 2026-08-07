// Native file/folder pickers (tauri-plugin-dialog). Wrapping here keeps the
// rest of the UI free of the plugin API and gives a single place to normalize
// `null` cancellations into `undefined`.

import { open, save } from "@tauri-apps/plugin-dialog";

/// Pick a single existing file (e.g. a txt/pdf/csv source). Returns the path,
/// or `undefined` when the user cancels.
export async function pickFile(options?: {
  title?: string;
  filters?: { name: string; extensions: string[] }[];
  defaultPath?: string;
}): Promise<string | undefined> {
  const picked = await open({
    multiple: false,
    directory: false,
    title: options?.title,
    filters: options?.filters,
    defaultPath: options?.defaultPath,
  });
  return typeof picked === "string" ? picked : undefined;
}

/// Pick an existing directory (e.g. an image folder). Returns the path, or
/// `undefined` on cancel.
export async function pickDirectory(options?: {
  title?: string;
  defaultPath?: string;
}): Promise<string | undefined> {
  const picked = await open({
    multiple: false,
    directory: true,
    title: options?.title,
    defaultPath: options?.defaultPath,
  });
  return typeof picked === "string" ? picked : undefined;
}

/// Choose a destination path (e.g. where to export). Returns the path, or
/// `undefined` on cancel.
export async function pickSavePath(options?: {
  title?: string;
  defaultPath?: string;
  filters?: { name: string; extensions: string[] }[];
}): Promise<string | undefined> {
  const picked = await save({
    title: options?.title,
    defaultPath: options?.defaultPath,
    filters: options?.filters,
  });
  return picked ?? undefined;
}
