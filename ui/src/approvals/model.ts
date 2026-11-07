import type { components } from "@/api/client";

type Request = components["schemas"]["ApprovalRequestInfo"];

export type ApprovalStatus = Request["status"];
export type ApprovalAction = Request["action"];

export const ACTION_LABEL: Record<ApprovalAction, string> = {
  publish: "Publish",
  publish_and_activate: "Publish and activate",
  activate: "Activate",
};

/** What a request shows as its state, and whether an admin can still decide it. */
export interface DisplayState {
  label: string;
  /** Why the request is in this state, for the review page. */
  reason: string;
  canDecide: boolean;
}

export function displayState(request: Pick<Request, "status" | "number">): DisplayState {
  switch (request.status) {
    case "pending":
      return { label: "Pending", reason: "Waiting for an admin to decide.", canDecide: true };
    case "approved":
      return {
        label: "Approved",
        reason: "An admin approved it and it took effect.",
        canDecide: false,
      };
    case "rejected":
      return { label: "Rejected", reason: "An admin rejected it.", canDecide: false };
    case "withdrawn":
      return {
        label: "Withdrawn",
        reason: "The requester withdrew it, or replaced it with a newer request for this Workflow.",
        canDecide: false,
      };
    case "stale":
      return {
        label: "Stale",
        reason: `The Draft changed after this request was made, so v${request.number} is no longer what the agent asked to publish. Nothing can be approved here; the agent has to ask again.`,
        canDecide: false,
      };
  }
}

/** The Pending tab holds requests still waiting; the Decided tab holds all the others. */
export type Tab = "pending" | "decided";

export function inTab(request: Pick<Request, "status">, tab: Tab): boolean {
  return (request.status === "pending") === (tab === "pending");
}

/** How long ago, in the largest whole unit: "just now", "5 min ago", "3 h ago", "2 d ago". */
export function age(createdAt: string, now: Date): string {
  const seconds = Math.max(0, Math.floor((now.getTime() - new Date(createdAt).getTime()) / 1000));
  if (seconds < 60) return "just now";
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  return `${Math.floor(hours / 24)} d ago`;
}
