import { useEffect, useState } from "react";
import { fetchFileBlob } from "@/lib/download";
import { Spinner } from "@/components/ui/spinner";

/** An image or PDF from the session's folder, shown from its bytes. */
export function FilePreview({ sessionId, path, kind, version }: { sessionId: string; path: string; kind: "image" | "pdf"; version?: number }) {
  const [state, setState] = useState<{ url?: string; error?: string }>({});
  useEffect(() => {
    let url: string | undefined;
    let cancelled = false;
    setState({});
    fetchFileBlob(sessionId, path).then(
      (blob) => {
        url = URL.createObjectURL(blob);
        if (cancelled) URL.revokeObjectURL(url);
        else setState({ url });
      },
      (e) => !cancelled && setState({ error: String(e?.message ?? e) }),
    );
    return () => {
      cancelled = true;
      if (url) URL.revokeObjectURL(url);
    };
  }, [sessionId, path, version]);

  if (state.error) return <div className="flex flex-1 items-center justify-center p-6 text-sm text-destructive">{state.error}</div>;
  if (!state.url)
    return (
      <div className="flex flex-1 items-center justify-center gap-2 p-6 text-sm text-muted-foreground">
        <Spinner /> Loading…
      </div>
    );
  return kind === "pdf" ? (
    <iframe src={state.url} title={path} className="min-h-0 w-full flex-1 border-0 bg-white" />
  ) : (
    <div className="flex min-h-0 flex-1 items-center justify-center overflow-auto p-4">
      <img src={state.url} alt={path} className="max-h-full max-w-full object-contain" />
    </div>
  );
}
