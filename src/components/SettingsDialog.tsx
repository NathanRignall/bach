import { Check, Contrast, Monitor, Moon, Sun } from "lucide-react";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { ACCENTS, MODES, Mode, setTheme, useTheme } from "@/lib/theme";
import { cn } from "@/lib/utils";

const MODE_ICONS: Record<Mode, typeof Sun> = { system: Monitor, light: Sun, dark: Moon, "high-contrast": Contrast };

/** App preferences. The theme applies as soon as it is picked and is remembered on this device. */
export function SettingsDialog({ onClose }: { onClose: () => void }) {
  const theme = useTheme();
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Settings</DialogTitle>
          <DialogDescription>Appearance is kept on this device.</DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-5">
          <div className="flex flex-col gap-2">
            <Label id="theme-mode">Theme</Label>
            <div role="radiogroup" aria-labelledby="theme-mode" className="grid grid-cols-2 gap-2">
              {MODES.map((m) => {
                const Icon = MODE_ICONS[m.value];
                const on = theme.mode === m.value;
                return (
                  <button
                    key={m.value}
                    role="radio"
                    aria-checked={on}
                    onClick={() => setTheme({ mode: m.value })}
                    className={cn(
                      "flex items-center justify-center gap-2 rounded-lg border px-3 py-2 text-sm hover:bg-accent",
                      on && "border-primary bg-primary/5",
                    )}
                  >
                    <Icon className="size-4" />
                    {m.label}
                  </button>
                );
              })}
            </div>
          </div>

          <div className={cn("flex flex-col gap-2", theme.mode === "high-contrast" && "opacity-40")}>
            <Label id="theme-accent">
              Accent colour
              {theme.mode === "high-contrast" && <span className="ml-1.5 font-normal text-muted-foreground">(unused in high contrast)</span>}
            </Label>
            <div
              role="radiogroup"
              aria-labelledby="theme-accent"
              aria-disabled={theme.mode === "high-contrast"}
              className={cn("flex flex-wrap gap-3", theme.mode === "high-contrast" && "pointer-events-none")}
            >
              {ACCENTS.map((a) => {
                const on = theme.accent === a.value;
                return (
                  <button
                    key={a.value}
                    role="radio"
                    aria-checked={on}
                    aria-label={a.label}
                    title={a.label}
                    onClick={() => setTheme({ accent: a.value })}
                    className={cn(
                      "flex size-8 items-center justify-center rounded-full text-white ring-offset-2 ring-offset-background outline-none focus-visible:ring-2 focus-visible:ring-ring",
                      on && "ring-2 ring-foreground/60",
                    )}
                    style={{ backgroundColor: a.swatch }}
                  >
                    {on && <Check className="size-4" />}
                  </button>
                );
              })}
            </div>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
