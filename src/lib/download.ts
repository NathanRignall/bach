import { readFileData } from "@/api";

/** A file's contents as a Blob, read through the backend (the file may be on another machine). */
export async function fetchFileBlob(sessionId: string, path: string): Promise<Blob> {
  const { mime, data } = await readFileData(sessionId, path);
  const bin = atob(data);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return new Blob([bytes], { type: mime });
}

/** Saves a file from the session's folder through the browser's download. */
export async function downloadFile(sessionId: string, path: string) {
  const url = URL.createObjectURL(await fetchFileBlob(sessionId, path));
  const a = document.createElement("a");
  a.href = url;
  a.download = path.slice(path.lastIndexOf("/") + 1);
  document.body.append(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}

const IMAGE = /\.(png|jpe?g|gif|webp|avif|bmp|ico)$/i;

/** How the file browser can show a file besides as text. */
export const previewKind = (path: string): "image" | "pdf" | undefined => (IMAGE.test(path) ? "image" : /\.pdf$/i.test(path) ? "pdf" : undefined);
