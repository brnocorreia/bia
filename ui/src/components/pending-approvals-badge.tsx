import { useApprovalRequests } from "@/api/approvals";

/** The number of requests waiting for a decision; nothing while there are none. */
export function PendingApprovalsBadge() {
  const count = useApprovalRequests({ status: "pending" }).data?.length ?? 0;
  if (count === 0) return null;
  return (
    <span
      aria-label={`${count} pending`}
      className="ml-2 inline-flex min-w-5 items-center justify-center rounded-full bg-primary px-1.5 text-xs font-medium text-primary-foreground"
    >
      {count}
    </span>
  );
}
