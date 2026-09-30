// Files attached to a message travel as `data:` URLs: images, PDFs and text files, which is
// what every agent can take. PDFs and text files carry their name as a parameter
// (`data:application/pdf;name=report.pdf;base64,…`); images don't need one.
//
// Claude scales anything with a longer edge than about 1568px down anyway, and rejects images
// over 5 MB, so big ones are shrunk here first: less to send over SSH, and nothing the API would
// refuse.

const MAX_EDGE = 1568;
const MAX_BYTES = 5 * 1024 * 1024;
/** What Claude accepts. */
const TYPES = ["image/png", "image/jpeg", "image/gif", "image/webp"];

/** Claude takes PDFs of up to 32 MB (and 100 pages); base64 makes them a third bigger. */
const MAX_PDF_BYTES = 20 * 1024 * 1024;
/** Text goes into the prompt whole, so a big file would crowd out everything else. */
const MAX_TEXT_BYTES = 1024 * 1024;

const readAsDataUrl = (blob: Blob) =>
  new Promise<string>((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(r.result as string);
    r.onerror = () => reject(r.error);
    r.readAsDataURL(blob);
  });

/** `file` as a `data:` URL the agents will take, or an error saying why it can't be attached. */
export async function toAttachment(file: File): Promise<string> {
  if (file.type.startsWith("image/")) return imageDataUrl(file);
  if (file.type === "application/pdf") {
    if (file.size > MAX_PDF_BYTES) throw new Error(`${file.name} is too big (PDFs can be up to 20 MB).`);
    return withName(await readAsDataUrl(file), file.name);
  }
  // Anything else is fine if it's text: browsers don't know most source files' types.
  const text = file.size <= MAX_TEXT_BYTES ? await asText(file) : undefined;
  if (text === undefined) {
    throw new Error(file.size > MAX_TEXT_BYTES ? `${file.name} is too big (text files can be up to 1 MB).` : `${file.name} isn't an image, PDF or text file.`);
  }
  if (!text.trim()) throw new Error(`${file.name} is empty.`);
  return withName(await readAsDataUrl(new Blob([text], { type: "text/plain" })), file.name);
}

/** `file`'s contents if they're UTF-8 text (and not binary that happens to decode). */
async function asText(file: File): Promise<string | undefined> {
  try {
    const text = new TextDecoder("utf-8", { fatal: true }).decode(await file.arrayBuffer());
    return text.includes("\0") ? undefined : text;
  } catch {
    return undefined;
  }
}

const withName = (url: string, name: string) => url.replace(/^data:([^;,]*)/, `data:$1;name=${encodeURIComponent(name)}`);

/** What an attachment is, for showing it: an image, or a file with its name. */
export function describe(url: string): { image: true } | { image: false; name: string; pdf: boolean } {
  const head = url.slice(5, url.indexOf(","));
  const [type, ...params] = head.split(";");
  if (type.startsWith("image/")) return { image: true };
  const name = params.find((p) => p.startsWith("name="))?.slice(5);
  const pdf = type === "application/pdf";
  return { image: false, name: name ? decodeURIComponent(name) : pdf ? "Document.pdf" : "Text file", pdf };
}

/** `file` as a `data:` URL Claude will take, scaled down if it's big. */
async function imageDataUrl(file: File): Promise<string> {
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
