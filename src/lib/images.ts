// Images attached to a message travel as `data:` URLs. Claude scales anything with a longer
// edge than about 1568px down anyway, and rejects images over 5 MB, so big ones are shrunk here
// first: less to send over SSH, and nothing the API would refuse.

const MAX_EDGE = 1568;
const MAX_BYTES = 5 * 1024 * 1024;
/** What Claude accepts. */
const TYPES = ["image/png", "image/jpeg", "image/gif", "image/webp"];

/** The image files among `files` (a drop or a paste). */
export const imageFiles = (files: FileList | File[]) => Array.from(files).filter((f) => f.type.startsWith("image/"));

const readAsDataUrl = (blob: Blob) =>
  new Promise<string>((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(r.result as string);
    r.onerror = () => reject(r.error);
    r.readAsDataURL(blob);
  });

/** `file` as a `data:` URL Claude will take, scaled down if it's big. */
export async function toDataUrl(file: File): Promise<string> {
  const bitmap = await createImageBitmap(file);
  const scale = Math.min(1, MAX_EDGE / Math.max(bitmap.width, bitmap.height));
  // Small enough as it is (a GIF stays as it is too, so it keeps its animation).
  if (TYPES.includes(file.type) && (file.type === "image/gif" || (scale === 1 && file.size <= MAX_BYTES))) {
    bitmap.close();
    return readAsDataUrl(file);
  }
  const canvas = document.createElement("canvas");
  canvas.width = Math.round(bitmap.width * scale);
  canvas.height = Math.round(bitmap.height * scale);
  canvas.getContext("2d")!.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
  bitmap.close();
  // PNG keeps screenshots crisp; photos and anything else become JPEG.
  const type = file.type === "image/png" ? "image/png" : "image/jpeg";
  const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, type, 0.9));
  if (!blob) throw new Error(`Couldn't read ${file.name}`);
  return readAsDataUrl(blob);
}
