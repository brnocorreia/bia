import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

const TONES: Record<string, string> = {
  succeeded: "border-transparent bg-emerald-100 text-emerald-900",
  failed: "border-transparent bg-red-100 text-red-900",
  cancelled: "border-transparent bg-zinc-200 text-zinc-800",
  skipped: "border-dashed bg-transparent text-muted-foreground",
  running: "border-transparent bg-blue-100 text-blue-900",
  pending: "border-transparent bg-amber-100 text-amber-900",
  sleeping: "border-transparent bg-amber-100 text-amber-900",
  waiting: "border-transparent bg-amber-100 text-amber-900",
};

/** A Run or Attempt status as a coloured label; the text always carries the meaning. */
export function StatusBadge({ status, className }: { status: string; className?: string }) {
  return (
    <Badge variant="outline" className={cn(TONES[status], className)}>
      {status}
    </Badge>
  );
}
