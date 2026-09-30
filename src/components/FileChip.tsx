import { FileText, FileType } from "lucide-react";
import { cn } from "@/lib/utils";

/** An attached PDF or text file, shown by its name (images get thumbnails instead). */
export function FileChip({ name, pdf, className }: { name: string; pdf: boolean; className?: string }) {
  const Icon = pdf ? FileType : FileText;
  return (
    <div title={name} className={cn("flex max-w-56 items-center gap-2 rounded-lg border bg-muted px-2.5 text-xs", className)}>
      <Icon className={cn("size-4 shrink-0", pdf ? "text-red-500" : "text-muted-foreground")} />
      <span className="truncate">{name}</span>
    </div>
  );
}
